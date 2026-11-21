use std::{
    cell::RefCell,
    collections::{HashSet, VecDeque},
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use nexssh_core::{
    Core, Destination, EventSink, Prompt, PtySize, Server, SessionEvent, SessionId, SessionStatus,
};
use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::{
    AppWindow, ServerRow, TabRow, i18n, prompts,
    services::{self, ServiceEvent},
    settings::Settings,
    settings_schema,
    terminal::{self, Terminal},
};

type SharedTerminal = Arc<Mutex<Terminal>>;

struct Sink {
    token: u64,
    terminal: SharedTerminal,
    events: mpsc::Sender<(u64, SessionEvent)>,
}

impl EventSink for Sink {
    fn event(&self, event: SessionEvent) {
        let _ = self.events.send((self.token, event));
    }
    fn output(&self, data: Vec<u8>) {
        // Keep raw output off the UI queue: memory is bounded by the parser's scrollback.
        self.terminal.lock().unwrap().process(&data);
    }
}

struct Tab {
    token: u64,
    id: SessionId,
    title: String,
    local: bool,
    server_id: Option<String>,
    status: SessionStatus,
    message: String,
    terminal: SharedTerminal,
    prompts: VecDeque<(u64, Prompt)>,
    saved_target: serde_json::Value,
    pending: Option<Target>,
}

enum Target {
    Ssh(Server),
    Local {
        command: nexssh_core::LocalCommand,
        title: String,
    },
}

enum Dialog {
    Auth {
        token: u64,
        id: u64,
    },
    Paste {
        session: SessionId,
        text: String,
    },
    Restore {
        backup: Box<nexssh_core::backup::Backup>,
    },
    Agent {
        id: u64,
    },
    RenewToken,
    InstallUpdate,
    RestoreTabs,
    DeleteServer {
        id: String,
        name: String,
    },
}

struct App {
    core: Arc<Core>,
    settings: Settings,
    services: services::Services,
    agents: crate::agents_ui::AgentUi,
    files: crate::files::Files,
    server_editor: crate::server_editor::Editor,
    startup_cwd: Option<std::path::PathBuf>,
    runtime: tokio::runtime::Runtime,
    events: mpsc::Receiver<(u64, SessionEvent)>,
    sender: mpsc::Sender<(u64, SessionEvent)>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    next_token: u64,
    filter: String,
    collapsed: HashSet<String>,
    dialog: Option<Dialog>,
    tabs_dirty: bool,
}

impl App {
    fn connect_server(&mut self, ui: &AppWindow, server: Server, new_session: bool) {
        if !new_session
            && let Some(index) = self
                .tabs
                .iter()
                .position(|t| t.server_id.as_deref() == Some(&server.id))
        {
            self.select(ui, index);
        } else {
            report(ui, self.open(ui, Some(server)));
        }
    }

    fn server_action(&mut self, ui: &AppWindow, action: &str, id: &str) {
        let lang = i18n::resolve(self.settings.text("language"));
        match action {
            "new" | "edit" => {
                let existing = if action == "edit" {
                    let Some(server) = self.core.store.get(id) else {
                        return;
                    };
                    Some(server)
                } else {
                    None
                };
                ui.set_settings_agents_enabled(self.settings.boolean("agentsEnabled"));
                ui.set_settings_keychain_ok(self.services.keychain_available);
                self.server_editor
                    .show(ui, Arc::clone(&self.core), existing, lang);
            }
            "cancel" => {
                if !ui.get_server_busy() {
                    self.server_editor.cancel(ui);
                    ui.invoke_close_overlay();
                }
            }
            "connect" | "new-session" => {
                if let Some(server) = self.core.store.get(id) {
                    self.connect_server(ui, server, action == "new-session");
                }
            }
            "duplicate" => {
                if let Some(mut server) = self.core.store.get(id) {
                    server.id.clear();
                    server.alias = None;
                    server.last_used = None;
                    server.name = i18n::tr("server.copyName", lang).replace("{name}", &server.name);
                    self.server_editor.duplicate(Arc::clone(&self.core), server);
                }
            }
            "copy-address" => {
                if let Some(server) = self.core.store.get(id) {
                    self.write_clipboard(ui, &server.destination(), "common.copied");
                }
            }
            "delete" => {
                if ui.get_server_busy() {
                    return;
                }
                if let Some(server) = self.core.store.get(id) {
                    if self.dialog.is_some() {
                        return;
                    }
                    ui.set_prompt_title(
                        i18n::tr("server.deleteTitle", lang)
                            .replace("{name}", &server.name)
                            .into(),
                    );
                    ui.set_prompt_detail(
                        i18n::tr(
                            if self.tabs.iter().any(|t| t.server_id.as_deref() == Some(id)) {
                                "server.deleteMessageOpen"
                            } else {
                                "server.deleteMessage"
                            },
                            lang,
                        )
                        .into(),
                    );
                    ui.set_prompt_accept_label(i18n::tr("common.delete", lang).into());
                    ui.set_prompt_fields(ModelRc::default());
                    ui.set_prompt_auth(false);
                    ui.set_prompt_host_key(false);
                    ui.set_prompt_can_accept(true);
                    ui.set_prompt_visible(true);
                    self.dialog = Some(Dialog::DeleteServer {
                        id: server.id,
                        name: server.name,
                    });
                }
            }
            _ => self
                .server_editor
                .action(ui, Arc::clone(&self.core), action, lang),
        }
    }

    fn server_editor_events(&mut self, ui: &AppWindow) {
        for event in self.server_editor.poll(ui) {
            match event {
                crate::server_editor::Outcome::Saved {
                    server,
                    connect,
                    access_changed,
                } => {
                    if access_changed {
                        self.agents.controller.forget_server(&server.id);
                    }
                    self.servers(ui);
                    if connect {
                        self.connect_server(ui, *server, true);
                    }
                }
                crate::server_editor::Outcome::Deleted { id, name } => {
                    self.agents.controller.forget_server(&id);
                    self.servers(ui);
                    if ui.get_overlay() == "server" && ui.get_server_form().id == id {
                        self.server_editor.cancel(ui);
                        ui.invoke_close_overlay();
                    }
                    if ui.get_notice().is_empty() {
                        ui.set_notice(
                            i18n::tr(
                                "server.deleted",
                                i18n::resolve(self.settings.text("language")),
                            )
                            .replace("{name}", &name)
                            .into(),
                        );
                    }
                }
            }
        }
        if ui.get_overlay() == "server" {
            self.server_editor.derived(
                ui,
                &self.core,
                i18n::resolve(self.settings.text("language")),
            );
        }
    }
    fn refresh_files(&self, ui: &AppWindow) {
        let mut entries = self
            .files
            .entries
            .iter()
            .filter(|entry| {
                self.settings.boolean("filesShowHidden") || !entry.name.starts_with('.')
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            let dir = |entry: &&nexssh_core::sftp::Entry| {
                entry.kind == nexssh_core::sftp::EntryKind::Dir || entry.link_to_dir
            };
            dir(b).cmp(&dir(a)).then_with(|| {
                let order = match self.settings.text("filesSort") {
                    "size" => a.size.cmp(&b.size),
                    "modified" => a.modified.cmp(&b.modified),
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                };
                let order = order.then_with(|| a.name.cmp(&b.name));
                if self.settings.boolean("filesSortDesc") {
                    order.reverse()
                } else {
                    order
                }
            })
        });
        ui.set_file_entries(ModelRc::new(VecModel::from(
            entries
                .into_iter()
                .map(|e| crate::FileRow {
                    name: e.name.as_str().into(),
                    path: nexssh_core::sftp::join(&self.files.path, &e.name).into(),
                    folder: e.kind == nexssh_core::sftp::EntryKind::Dir || e.link_to_dir,
                    size: if e.kind == nexssh_core::sftp::EntryKind::Dir {
                        String::new()
                    } else {
                        format_size(e.size)
                    }
                    .into(),
                    modified: e.modified.map(|n| n.to_string()).unwrap_or_default().into(),
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_files_hidden(self.settings.boolean("filesShowHidden"));
    }
    fn file_action(&mut self, ui: &AppWindow, key: &str, path: &str) {
        if key == "close" {
            ui.set_files_open(false);
            self.settings.set_value("filesOpen", false.into());
            return;
        }
        let Some(tab) = self
            .active()
            .filter(|t| !t.local && t.status == SessionStatus::Connected)
        else {
            ui.set_notice(
                i18n::tr(
                    "files.notConnected",
                    i18n::resolve(self.settings.text("language")),
                )
                .into(),
            );
            return;
        };
        let session = tab.id;
        let initial_path = tab
            .server_id
            .as_deref()
            .and_then(|id| self.core.store.get(id))
            .and_then(|s| s.sftp_root)
            .unwrap_or_default();
        match key {
            "toggle" => {
                ui.set_files_open(!ui.get_files_open());
                self.settings
                    .set_value("filesOpen", ui.get_files_open().into());
                if ui.get_files_open() {
                    ui.set_files_loading(true);
                    self.files.list(
                        Arc::clone(&self.core),
                        &self.runtime,
                        session,
                        if self.files.session == session {
                            self.files.path.clone()
                        } else {
                            initial_path
                        },
                    );
                }
            }
            "list" | "up" => {
                let path = if key == "up" {
                    self.files
                        .path
                        .rsplit_once('/')
                        .map_or("/", |(p, _)| if p.is_empty() { "/" } else { p })
                        .to_owned()
                } else {
                    path.to_owned()
                };
                ui.set_files_loading(true);
                self.files
                    .list(Arc::clone(&self.core), &self.runtime, session, path);
            }
            "open" => {
                let name = nexssh_core::sftp::base_name(path);
                if self.files.entries.iter().any(|e| {
                    e.name == name && (e.kind == nexssh_core::sftp::EntryKind::Dir || e.link_to_dir)
                }) {
                    self.file_action(ui, "list", path);
                } else {
                    self.file_action(
                        ui,
                        if self.settings.text("filesDoubleClick") == "edit" {
                            "edit"
                        } else {
                            "download"
                        },
                        path,
                    );
                }
            }
            "download" => self
                .files
                .download(Arc::clone(&self.core), &self.runtime, path.into()),
            "upload" | "upload-folder" => self.files.upload(
                Arc::clone(&self.core),
                &self.runtime,
                key == "upload-folder",
            ),
            "edit" => self
                .files
                .edit(&self.runtime, path.into(), self.settings.value.clone()),
            "cancel" => self.files.cancel(),
            "edit-overwrite" | "edit-reload" => {
                if let Ok(id) = path.parse() {
                    self.files
                        .resolve(&self.runtime, id, key == "edit-overwrite");
                }
            }
            _ => {}
        }
    }
    fn file_events(&mut self, ui: &AppWindow) {
        while let Ok(event) = self.files.receiver.try_recv() {
            match event {
                crate::files::Event::Listing {
                    generation,
                    path,
                    result,
                } => {
                    if generation != self.files.generation {
                        continue;
                    }
                    ui.set_files_loading(false);
                    self.files.path = path;
                    ui.set_files_path(self.files.path.as_str().into());
                    match result {
                        Ok(entries) => self.files.entries = entries,
                        Err(error) => ui.set_notice(error.into()),
                    };
                    self.refresh_files(ui);
                }
                crate::files::Event::Message(message) => {
                    self.files.transferring = false;
                    ui.set_file_transfer("".into());
                    if !message.is_empty() {
                        ui.set_notice(message.into());
                    }
                    if ui.get_files_open() {
                        let path = self.files.path.clone();
                        self.file_action(ui, "list", &path);
                    }
                }
                crate::files::Event::Transfer(progress) => ui.set_file_transfer(progress.into()),
                crate::files::Event::Edits(list) => {
                    ui.set_file_edits(ModelRc::new(VecModel::from(
                        list.iter()
                            .map(|edit| crate::EditRow {
                                id: edit.id.to_string().into(),
                                name: edit.name.as_str().into(),
                                status: edit
                                    .error
                                    .clone()
                                    .unwrap_or_else(|| format!("{:?}", edit.state))
                                    .into(),
                                conflict: edit.state == nexssh_core::edit::EditState::Conflict,
                            })
                            .collect::<Vec<_>>(),
                    )));
                    self.files.editing = list;
                }
            }
        }
        if ui.get_files_open()
            && let Some(tab) = self
                .active()
                .filter(|t| !t.local && t.status == SessionStatus::Connected)
            && self.files.session != tab.id
        {
            let session = tab.id;
            let initial_path = tab
                .server_id
                .as_deref()
                .and_then(|id| self.core.store.get(id))
                .and_then(|s| s.sftp_root)
                .unwrap_or_default();
            ui.set_files_loading(true);
            self.files
                .list(Arc::clone(&self.core), &self.runtime, session, initial_path);
        }
    }
    fn snapshot_tabs(&mut self, reopen: bool) {
        let saved = if reopen {
            serde_json::json!({"active":self.active.map_or(-1,|i|i as i32),"tabs":self.tabs.iter().map(|t|serde_json::json!({"kind":if t.local {"local"}else{"ssh"},"target":t.saved_target,"title":t.title,"subtitle":""})).collect::<Vec<_>>()})
        } else {
            serde_json::Value::Null
        };
        self.settings.set_value("lastTabs", saved);
    }
    fn request_close(&mut self, ui: &AppWindow) {
        match self.settings.text("closeAction") {
            "tray" if ui.get_tray_available() => {
                report(ui, ui.hide());
            }
            "quit" => self.decide_close(
                ui,
                "quit",
                false,
                self.settings.text("restoreTabs") == "always",
            ),
            _ => {
                ui.set_close_reopen(self.settings.text("restoreTabs") != "never");
                ui.set_overlay("close".into());
            }
        }
    }
    fn decide_close(&mut self, ui: &AppWindow, action: &str, remember: bool, reopen: bool) {
        if remember {
            self.settings.set_value("closeAction", action.into());
        }
        ui.invoke_close_overlay();
        if action == "tray" && ui.get_tray_available() {
            report(ui, ui.hide());
            return;
        }
        self.snapshot_tabs(match self.settings.text("restoreTabs") {
            "always" => true,
            "never" => false,
            _ => reopen,
        });
        report(ui, self.settings.flush());
        report(ui, slint::quit_event_loop());
    }
    fn restore_tabs(&mut self, ui: &AppWindow) {
        let saved = self.settings.value["lastTabs"].clone();
        for item in saved["tabs"].as_array().into_iter().flatten().take(50) {
            let target = &item["target"];
            let pending = if item["kind"] == "ssh" {
                target["serverId"]
                    .as_str()
                    .and_then(|id| self.core.store.get(id))
                    .or_else(|| {
                        target["destination"]
                            .as_str()
                            .and_then(|d| Destination::parse(d).ok())
                            .map(|d| d.to_server())
                    })
                    .map(Target::Ssh)
            } else if let Some(command) = target["local"]["command"].as_str() {
                nexssh_core::LocalCommand::parse(
                    command,
                    target["local"]["cwd"].as_str().map(std::path::Path::new),
                )
                .ok()
                .map(|command| Target::Local {
                    command,
                    title: item["title"].as_str().unwrap_or("Terminal").into(),
                })
            } else {
                let profiles = nexssh_core::local::shells();
                let selected = target["local"]["profile"].as_str().unwrap_or("");
                profiles
                    .iter()
                    .find(|p| p.id == selected)
                    .or_else(|| profiles.first())
                    .map(|p| Target::Local {
                        command: p
                            .command(target["local"]["cwd"].as_str().map(std::path::Path::new)),
                        title: p.name.clone(),
                    })
            };
            if let Some(pending) = pending {
                let token = self.next_token;
                self.next_token += 1;
                let (local, server_id) = match &pending {
                    Target::Ssh(s) => (false, Some(s.id.clone())),
                    _ => (true, None),
                };
                let size = size(ui);
                self.tabs.push(Tab {
                    token,
                    id: 0,
                    title: item["title"].as_str().unwrap_or("Terminal").into(),
                    local,
                    server_id,
                    status: SessionStatus::Disconnected,
                    message: i18n::tr(
                        "restore.pressEnter",
                        i18n::resolve(self.settings.text("language")),
                    ),
                    terminal: Arc::new(Mutex::new(Terminal::with_scrollback(
                        size.rows as u16,
                        size.cols as u16,
                        self.settings.number("scrollback") as usize,
                    ))),
                    prompts: VecDeque::new(),
                    saved_target: target.clone(),
                    pending: Some(pending),
                });
            }
        }
        let index = saved["active"].as_i64().unwrap_or(-1);
        if index >= 0 && !self.tabs.is_empty() {
            self.select(ui, (index as usize).min(self.tabs.len() - 1));
        }
        self.tabs_dirty = true;
    }
    fn reconnect_active(&mut self, ui: &AppWindow) {
        let Some(index) = self.active else { return };
        if let Some(target) = self.tabs[index].pending.take() {
            self.tabs.remove(index);
            self.start(ui, target);
            let tab = self.tabs.pop().unwrap();
            self.tabs.insert(index, tab);
            self.select(ui, index);
        } else {
            report(ui, self.core.sessions.reconnect(self.tabs[index].id));
        }
    }
    fn agent_events(&mut self, ui: &AppWindow) {
        while let Ok(event) = self.agents.events.try_recv() {
            match event {
                crate::agents::Event::Attention => {
                    let _ = ui.show();
                    ui.window().set_minimized(false);
                }
                crate::agents::Event::Status(status) => {
                    let lang = i18n::resolve(self.settings.text("language"));
                    self.agents.summary = if let Some(error) = &status.error {
                        error.clone()
                    } else if status.running {
                        i18n::tr("agents.running", lang).replace("{url}", &status.url)
                    } else if status.enabled {
                        i18n::tr("agents.starting", lang)
                    } else {
                        i18n::tr("editor.agentsOff", lang)
                    };
                    ui.set_agents_activity(
                        status
                            .activity
                            .iter()
                            .map(|a| {
                                format!(
                                    "{} · {} · {:?}\n{}",
                                    a.agent, a.server_name, a.status, a.detail
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n\n")
                            .into(),
                    );
                    self.agents.status = *status;
                    if ui.get_overlay() == "settings" && ui.get_settings_section() == "agents" {
                        self.refresh_settings(ui);
                    }
                }
                crate::agents::Event::Read(request) => {
                    let server = request["serverId"].as_str().unwrap_or("");
                    let tabs = self
                        .tabs
                        .iter()
                        .filter(|t| t.server_id.as_deref() == Some(server))
                        .collect::<Vec<_>>();
                    let requested = request["tab"].as_u64();
                    let selected = if let Some(index) = requested {
                        tabs.get(index.saturating_sub(1) as usize).copied()
                    } else {
                        self.active()
                            .filter(|t| t.server_id.as_deref() == Some(server))
                            .or_else(|| tabs.last().copied())
                    };
                    let screen = selected.map(|tab| {
                        let mut terminal = tab.terminal.lock().unwrap();
                        let screen = terminal.parser.screen();
                        let (rows, cols) = screen.size();
                        let (row, col) = screen.cursor_position();
                        let mut result = crate::agents::Screen {
                            title: tab.title.clone(),
                            status: match tab.status {
                                SessionStatus::Connected => "connected",
                                SessionStatus::Connecting => "connecting",
                                _ => "disconnected",
                            }
                            .into(),
                            tab: tabs.iter().position(|t| t.token == tab.token).unwrap_or(0) as u32
                                + 1,
                            tabs: tabs.len() as u32,
                            cols: u32::from(cols.get()),
                            rows: u32::from(rows.get()),
                            cursor_row: u32::from(row) + 1,
                            cursor_col: u32::from(col) + 1,
                            alternate: screen.alternate_screen(),
                            above: screen.scrollback_len() as u32,
                            from: 1,
                            total: 0,
                            text: String::new(),
                        };
                        result.text = terminal.history(
                            request["lines"]
                                .as_u64()
                                .map_or(rows.get() as usize, |n| n.clamp(1, 5000) as usize),
                        );
                        result.total = result.text.lines().count() as u32;
                        result
                    });
                    self.agents.controller.screen_read(
                        request["id"].as_u64().unwrap_or(0),
                        screen,
                        Some(
                            if requested.is_some() {
                                "noSuchTab"
                            } else {
                                "noTab"
                            }
                            .into(),
                        ),
                    );
                }
            }
        }
        for open in self.agents.status.opens.clone() {
            if self.agents.opened.contains_key(&open.id) {
                continue;
            }
            if let Some(server) = self.core.store.get(&open.server_id) {
                let token = self.next_token;
                match self.open(ui, Some(server)) {
                    Ok(()) => {
                        self.agents.opened.insert(open.id, token);
                    }
                    Err(error) => self
                        .agents
                        .controller
                        .open_failed(open.id, error.to_string()),
                }
            } else {
                self.agents
                    .controller
                    .open_failed(open.id, "Server removed".into());
            }
        }
        self.agents.opened.retain(|id, token| {
            if !self.agents.status.opens.iter().any(|o| o.id == *id) {
                return false;
            }
            if let Some(tab) = self.tabs.iter().find(|t| t.token == *token) {
                if matches!(
                    tab.status,
                    SessionStatus::Disconnected | SessionStatus::Closed
                ) {
                    self.agents.controller.open_failed(*id, tab.message.clone());
                    return false;
                }
            } else {
                self.agents.controller.open_failed(*id, "Tab closed".into());
                return false;
            }
            true
        });
        if let Some(Dialog::Agent { id }) = self.dialog
            && !self.agents.status.requests.iter().any(|r| r.id == id)
        {
            self.dialog = None;
            prompts::clear(ui);
        }
        if self.dialog.is_none()
            && let Some(request) = self.agents.status.requests.first()
        {
            ui.set_prompt_title(format!("{} · {}", request.agent, request.server_name).into());
            ui.set_prompt_detail(
                format!(
                    "{}\n{}\n{}",
                    request.tool,
                    request.detail,
                    request.preview.as_deref().unwrap_or("")
                )
                .into(),
            );
            ui.set_prompt_fields(ModelRc::default());
            ui.set_prompt_host_key(false);
            ui.set_prompt_auth(false);
            ui.set_prompt_can_accept(true);
            ui.set_prompt_visible(true);
            self.dialog = Some(Dialog::Agent { id: request.id });
        }
    }

    fn refresh_saved_colors(&self, ui: &AppWindow) {
        let preset = crate::settings::defaults();
        let values = crate::highlight::KINDS
            .iter()
            .filter_map(|kind| preset["highlightColors"][kind].as_str())
            .chain(["#8c929b"]);
        let swatch = |hex: &str| crate::ColorSwatch {
            hex: hex.into(),
            color: crate::colors::parse(hex).unwrap_or_default().color,
        };
        ui.set_settings_color_presets(ModelRc::new(VecModel::from(
            values.map(swatch).collect::<Vec<_>>(),
        )));
        ui.set_settings_saved_colors(ModelRc::new(VecModel::from(
            self.settings.value["savedColors"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .filter(|c| crate::settings::valid_color(c))
                .take(14)
                .map(swatch)
                .collect::<Vec<_>>(),
        )));
    }
    fn agent_snippet(&self, client: &str) -> String {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let json = serde_json::to_string(&exe).unwrap();
        match client {
            "claude"=>format!("claude mcp add --scope user nexssh\n  {json} mcp"),
            "codex"=>format!("[mcp_servers.nexssh]\ncommand = {json}\nargs = [\"mcp\"]"),
            "http"=>serde_json::to_string_pretty(&serde_json::json!({"mcpServers":{"nexssh":{"type":"http","url":self.agents.status.url,"headers":{"Authorization":"Bearer <token>"}}}})).unwrap(),
            _=>serde_json::to_string_pretty(&serde_json::json!({"mcpServers":{"nexssh":{"command":exe,"args":["mcp"]}}})).unwrap(),
        }
    }
    fn refresh_settings(&self, ui: &AppWindow) {
        let update_status = self
            .services
            .updater
            .status(i18n::resolve(self.settings.text("language")));
        let snapshot = self.core.store.snapshot();
        let context = settings_schema::Context {
            shells: &self.services.shells,
            editors: &self.services.editors,
            data_dir: &self.core.data_dir,
            servers: &snapshot.servers,
            agents_status: &self.agents.summary,
            update_status: &update_status,
            keychain_status: &self.services.keychain,
        };
        ui.set_settings_rows(settings_schema::rows(
            &self.settings,
            &ui.get_settings_section(),
            &context,
        ));
        let associations = self.settings.value["editorAssociations"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| crate::AssociationRow {
                extensions: v["extensions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                editor: v["editor"].as_str().unwrap_or("system").into(),
                command: v["command"].as_str().unwrap_or("").into(),
            })
            .collect::<Vec<_>>();
        ui.set_settings_associations(ModelRc::new(VecModel::from(associations)));
        let lang = i18n::resolve(self.settings.text("language"));
        let mut editors = vec![crate::Choice {
            value: "system".into(),
            label: i18n::tr("settings.editorSystem", lang).into(),
        }];
        editors.extend(self.services.editors.iter().map(|(v, l)| crate::Choice {
            value: v.into(),
            label: l.into(),
        }));
        editors.push(crate::Choice {
            value: "custom".into(),
            label: i18n::tr("settings.editorCustom", lang).into(),
        });
        ui.set_settings_editors(ModelRc::new(VecModel::from(editors)));
        ui.set_settings_data_path(self.core.data_dir.display().to_string().into());
        ui.set_settings_version(
            i18n::tr("settings.version", lang)
                .replace("{version}", env!("CARGO_PKG_VERSION"))
                .into(),
        );
        ui.set_settings_update_status(update_status.as_str().into());
        ui.set_settings_update_action(self.services.updater.label(lang).into());
        ui.set_settings_update_enabled(self.services.updater.enabled());
        ui.set_update_offer(self.services.updater.offer());
        ui.set_settings_keychain_status(self.services.keychain.as_str().into());
        ui.set_settings_keychain_ok(self.services.keychain_available);
        ui.set_settings_agents_summary(self.agents.summary.as_str().into());
        ui.set_settings_agents_error(self.agents.status.error.is_some());
        ui.set_settings_share_options(ModelRc::new(VecModel::from(
            snapshot
                .servers
                .iter()
                .filter(|s| s.agents == nexssh_core::AgentAccess::Off)
                .map(|s| crate::Choice {
                    value: format!("agent-server:{}", s.id).into(),
                    label: s.name.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_settings_agents_enabled(self.settings.boolean("agentsEnabled"));
        ui.set_settings_snippets(ModelRc::new(VecModel::from(
            ["claude", "codex", "json", "http"]
                .into_iter()
                .map(|client| crate::Choice {
                    value: client.into(),
                    label: self.agent_snippet(client).into(),
                })
                .collect::<Vec<_>>(),
        )));
        self.refresh_saved_colors(ui);
    }

    fn patch_setting(&mut self, ui: &AppWindow, key: &str, value: &str) {
        if let Some(id) = key.strip_prefix("agent-server:") {
            if let Some(mut server) = self.core.store.get(id) {
                server.agents = match value {
                    "ask" => nexssh_core::AgentAccess::Ask,
                    "allow" => nexssh_core::AgentAccess::Allow,
                    _ => nexssh_core::AgentAccess::Off,
                };
                report(ui, self.core.store.save_server(server));
                self.agents.controller.forget_server(id);
                self.refresh_settings(ui);
            }
            return;
        }
        if let Err(error) = self.settings.set(key, value) {
            ui.set_notice(error.into());
            return;
        }
        self.settings.apply(ui);
        if key.starts_with("files") {
            self.refresh_files(ui);
        }
        if key == "sidebarAutoHide" {
            ui.set_sidebar_hidden(if self.settings.boolean(key) && self.active.is_some() {
                true
            } else {
                self.settings.boolean("sidebarHidden")
            });
        }
        if [
            "language",
            "localShell",
            "editor",
            "highlightKeywords",
            "agentsEnabled",
        ]
        .contains(&key)
        {
            self.refresh_settings(ui);
        }
        if key == "explorerMenu" {
            let enabled = self.settings.boolean(key);
            self.services.job(move || {
                crate::explorer::apply(enabled)?;
                Ok(None)
            });
        }
        if key == "autoUpdateCheck" && self.settings.boolean(key) {
            self.services.check_updates();
        }
        if key == "agentsEnabled" {
            let _entered = self.runtime.enter();
            self.agents.controller.apply(&self.settings.value);
        }
        for tab in &self.tabs {
            tab.terminal.lock().unwrap().dirty = true;
        }
    }

    fn association(
        &mut self,
        ui: &AppWindow,
        index: i32,
        extensions: &str,
        editor: &str,
        command: &str,
    ) {
        let mut rules = self.settings.value["editorAssociations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let extensions = extensions
            .split([',', ';', ' '])
            .map(|s| s.trim().trim_start_matches("*.").to_lowercase())
            .filter(|s| !s.is_empty())
            .collect::<std::collections::BTreeSet<_>>();
        if index >= 0 && (index as usize) < rules.len() {
            rules.remove(index as usize);
        }
        if !extensions.is_empty() {
            for rule in &mut rules {
                if let Some(list) = rule["extensions"].as_array_mut() {
                    list.retain(|v| !extensions.contains(v.as_str().unwrap_or("")));
                }
            }
            rules.retain(|r| r["extensions"].as_array().is_some_and(|a| !a.is_empty()));
            rules.push(
                serde_json::json!({"extensions":extensions,"editor":editor,"command":command}),
            );
        }
        self.settings.set_value("editorAssociations", rules.into());
        self.refresh_settings(ui);
    }

    fn setting_action(&mut self, ui: &AppWindow, key: &str, value: &str) {
        self.settings.capture(ui);
        match key {
            "font-smaller" | "font-larger" => {
                let next = (self.settings.number("fontSize")
                    + if key == "font-smaller" { -1.0 } else { 1.0 })
                .clamp(9.0, 28.0);
                self.patch_setting(ui, "fontSize", &next.to_string());
                self.refresh_settings(ui);
            }
            "agents-hide-token" => ui.set_settings_agent_token("".into()),
            "agents-reveal-token" => {
                let agents = Arc::clone(&self.agents.controller);
                let core = Arc::clone(&self.core);
                self.services.job(move || {
                    Ok(Some(ServiceEvent::AgentToken(
                        agents.token_blocking(&core.secrets),
                    )))
                });
            }
            "backup-export" => self.services.export(
                Arc::clone(&self.core),
                self.settings.value.clone(),
                value.into(),
            ),
            "backup-import" => self.services.import(),
            "browse-shell" => self.services.browse("localShellCommand"),
            "browse-editor" => self.services.browse("editorCommand"),
            "copy-data-path" => self.write_clipboard(
                ui,
                &self.core.data_dir.display().to_string(),
                "common.copied",
            ),
            "open-data-path" => self.services.open(self.core.data_dir.clone()),
            "project-home" => self.services.url("https://github.com/likDanil/NexSSH"),
            "logs-show" => {
                if let Some(dir) = crate::logger::dir() {
                    self.services.open(dir)
                }
            }
            "logs-export" => self.services.logs_export(),
            "updates-check" => {
                if self.services.updater.ready() {
                    if self.dialog.is_some() {
                        return;
                    }
                    let lang = i18n::resolve(self.settings.text("language"));
                    ui.set_prompt_title(i18n::tr("update.confirmTitle", lang).into());
                    ui.set_prompt_detail(i18n::tr("update.confirmMessage", lang).into());
                    ui.set_prompt_auth(false);
                    ui.set_prompt_fields(ModelRc::default());
                    ui.set_prompt_host_key(false);
                    ui.set_prompt_can_accept(true);
                    self.dialog = Some(Dialog::InstallUpdate);
                    ui.set_prompt_visible(true);
                } else {
                    self.services.updater.download_or_check();
                }
                self.refresh_settings(ui);
            }
            "agents-clear" => self.agents.controller.clear_activity(),
            "agents-renew" => {
                let lang = i18n::resolve(self.settings.text("language"));
                ui.set_prompt_title(i18n::tr("agents.newTokenTitle", lang).into());
                ui.set_prompt_detail(i18n::tr("agents.newTokenConfirm", lang).into());
                ui.set_prompt_fields(ModelRc::default());
                ui.set_prompt_auth(false);
                ui.set_prompt_host_key(false);
                ui.set_prompt_can_accept(true);
                ui.set_prompt_visible(true);
                self.dialog = Some(Dialog::RenewToken);
            }
            "agents-token" => {
                let agents = Arc::clone(&self.agents.controller);
                let core = Arc::clone(&self.core);
                self.services.job(move || {
                    Ok(Some(ServiceEvent::Clipboard(
                        agents.token_blocking(&core.secrets),
                    )))
                });
            }
            key if key.starts_with("agents-copy-") => {
                let exe = std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let json = serde_json::to_string(&exe).unwrap();
                let text=match key {
                    "agents-copy-claude"=>format!("claude mcp add --scope user nexssh {json} mcp"),
                    "agents-copy-codex"=>format!("[mcp_servers.nexssh]\ncommand = {json}\nargs = [\"mcp\"]"),
                    "agents-copy-http"=>serde_json::json!({"mcpServers":{"nexssh":{"type":"http","url":self.agents.status.url,"headers":{"Authorization":"Bearer <token>"}}}}).to_string(),
                    _=>serde_json::json!({"mcpServers":{"nexssh":{"command":exe,"args":["mcp"]}}}).to_string(),
                };
                self.write_clipboard(ui, &text, "agents.copied");
            }
            _ => {}
        }
    }

    fn service_events(&mut self, ui: &AppWindow) {
        if self.services.updater.poll() {
            self.refresh_settings(ui);
        }
        while let Ok(event) = self.services.receiver.try_recv() {
            let lang = i18n::resolve(self.settings.text("language"));
            match event {
                ServiceEvent::AgentToken(token) => {
                    if ui.get_overlay() == "settings" && ui.get_settings_section() == "agents" {
                        ui.set_settings_agent_token(token.into());
                    }
                }
                ServiceEvent::Clipboard(text) => self.write_clipboard(ui, &text, "agents.copied"),
                ServiceEvent::Metadata {
                    shells,
                    editors,
                    keychain,
                } => {
                    self.services.keychain_available = keychain.is_none();
                    self.services.shells = shells;
                    self.services.editors = editors;
                    self.services.keychain = keychain
                        .map(|reason| {
                            i18n::tr("settings.keychainBad", lang).replace("{reason}", &reason)
                        })
                        .unwrap_or_else(|| i18n::tr("settings.keychainOk", lang));
                }
                ServiceEvent::Notice(message) => ui.set_notice(message.into()),
                ServiceEvent::Preference(key, value) => {
                    self.patch_setting(ui, key, &value);
                    self.refresh_settings(ui);
                }
                ServiceEvent::Chosen(backup) => {
                    // A backup replaces the current collection; show exactly what will be replaced first.
                    if self.dialog.is_some() {
                        ui.set_notice("Сначала завершите текущий диалог".into());
                        continue;
                    }
                    let summary = backup.summary();
                    ui.set_prompt_title(i18n::tr("backup.confirmTitle", lang).into());
                    ui.set_prompt_detail(
                        format!(
                            "NexSSH {} · {} серверов · {} групп\n{}\n{} вкладок будут закрыты.",
                            summary.app_version,
                            summary.servers,
                            summary.groups,
                            i18n::tr("backup.confirmReplace", lang),
                            self.tabs.len()
                        )
                        .into(),
                    );
                    ui.set_prompt_fields(ModelRc::new(VecModel::from(if summary.passwords {
                        vec![crate::PromptField {
                            label: i18n::tr("backup.passwordAsk", lang).into(),
                            secret: true,
                            value: "".into(),
                        }]
                    } else {
                        vec![]
                    })));
                    ui.set_prompt_host_key(false);
                    ui.set_prompt_auth(false);
                    ui.set_prompt_can_accept(true);
                    ui.set_prompt_visible(true);
                    self.dialog = Some(Dialog::Restore { backup });
                }
                ServiceEvent::RestorePrepared { backup, passwords } => match passwords {
                    Ok(passwords) => {
                        self.files.cancel();
                        self.files.edits.stop_all();
                        self.core.sessions.close_all();
                        self.tabs.clear();
                        self.active = None;
                        self.tabs_dirty = true;
                        self.services
                            .restore(Arc::clone(&self.core), *backup, passwords);
                    }
                    Err(error) => {
                        ui.set_notice(error.into());
                        ui.set_prompt_fields(ModelRc::new(VecModel::from(vec![
                            crate::PromptField {
                                label: i18n::tr("backup.passwordAsk", lang).into(),
                                secret: true,
                                value: "".into(),
                            },
                        ])));
                        ui.set_prompt_visible(true);
                        self.dialog = Some(Dialog::Restore { backup });
                    }
                },
                ServiceEvent::Restored {
                    settings: value,
                    failed,
                } => {
                    self.settings.replace(value);
                    self.settings.apply(ui);
                    self.collapsed = self.settings.value["collapsedGroups"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                    self.servers(ui);
                    ui.set_notice(
                        if failed == 0 {
                            i18n::tr("backup.restored", lang)
                        } else {
                            format!(
                                "{} · {failed} passwords failed",
                                i18n::tr("backup.restored", lang)
                            )
                        }
                        .into(),
                    );
                    let _entered = self.runtime.enter();
                    self.agents.controller.apply(&self.settings.value);
                }
            }
            if ui.get_overlay() == "settings" {
                self.refresh_settings(ui);
            }
        }
    }

    fn servers(&self, ui: &AppWindow) {
        let filter = self.filter.to_lowercase();
        let snapshot = self.core.store.snapshot();
        let rows: Vec<ServerRow> = snapshot
            .servers
            .into_iter()
            .map(|s| ServerRow {
                id: s.id.clone().into(),
                name: s.name.clone().into(),
                address: s.destination().into(),
                host: s.host.clone().into(),
                group: s.group.clone().into(),
                open_session: self
                    .tabs
                    .iter()
                    .any(|t| t.server_id.as_deref() == Some(&s.id)),
                state: self
                    .tabs
                    .iter()
                    .filter(|t| t.server_id.as_deref() == Some(&s.id))
                    .map(|t| state_code(t.status))
                    .max()
                    .unwrap_or(0),
                auth: match s.auth {
                    nexssh_core::AuthKind::Auto => "Авто",
                    nexssh_core::AuthKind::Password => "Пароль",
                    nexssh_core::AuthKind::Key => "Ключ",
                    nexssh_core::AuthKind::Agent => "SSH-агент",
                }
                .into(),
                ..ServerRow::default()
            })
            .collect();
        let mut groups = snapshot.groups;
        for row in &rows {
            if !groups.iter().any(|g| g == row.group.as_str()) {
                groups.push(row.group.to_string());
            }
        }
        let group_rows = |rows: &[ServerRow], collapse: bool| {
            let mut grouped = vec![];
            for group in &groups {
                let members: Vec<_> = rows
                    .iter()
                    .filter(|row| row.group == group.as_str())
                    .collect();
                if members.is_empty() && collapse && !filter.is_empty() {
                    continue;
                }
                let collapsed = collapse && filter.is_empty() && self.collapsed.contains(group);
                if !group.is_empty() || groups.len() > 1 {
                    grouped.push(ServerRow {
                        header: true,
                        group: group.as_str().into(),
                        name: if group.is_empty() {
                            "Без группы"
                        } else {
                            group
                        }
                        .into(),
                        count: members.len() as i32,
                        collapsed,
                        ..ServerRow::default()
                    });
                }
                grouped.extend(members.into_iter().map(|row| {
                    let mut row = row.clone();
                    row.hidden = collapsed;
                    row
                }));
            }
            grouped
        };
        let filtered: Vec<_> = rows
            .iter()
            .filter(|row| matches_server(row, &filter))
            .cloned()
            .collect();
        ui.set_grouped_servers(ModelRc::new(VecModel::from(group_rows(&filtered, true))));
        ui.set_home_servers(ModelRc::new(VecModel::from(group_rows(&rows, false))));
        ui.set_palette_servers(ModelRc::new(VecModel::from(rows.clone())));
        ui.set_servers(ModelRc::new(VecModel::from(rows)));
        ui.set_filter_active(!filter.is_empty());
    }

    fn active(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    fn open(
        &mut self,
        ui: &AppWindow,
        server: Option<Server>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = if server.is_none() {
            self.startup_cwd.take()
        } else {
            None
        };
        let target = if let Some(server) = server {
            Target::Ssh(server)
        } else {
            if self.settings.text("localShell") == "custom" {
                Target::Local {
                    command: nexssh_core::LocalCommand::parse(
                        self.settings.text("localShellCommand"),
                        cwd.as_deref(),
                    )?,
                    title: i18n::tr(
                        "settings.shellCustom",
                        i18n::resolve(self.settings.text("language")),
                    ),
                }
            } else {
                let profiles = if self.services.shells.is_empty() {
                    nexssh_core::local::shells()
                } else {
                    self.services.shells.clone()
                };
                let selected = self.settings.text("localShell");
                let profile = if selected.is_empty() {
                    profiles.first()
                } else {
                    profiles.iter().find(|s| s.id == selected)
                }
                .ok_or("Выбранная оболочка не установлена")?;
                Target::Local {
                    command: profile.command(cwd.as_deref()),
                    title: profile.name.clone(),
                }
            }
        };
        self.start(ui, target);
        Ok(())
    }

    fn start(&mut self, ui: &AppWindow, target: Target) {
        let saved_target = match &target {
            Target::Ssh(s) if self.core.store.get(&s.id).is_some() => {
                serde_json::json!({"serverId":s.id})
            }
            Target::Ssh(s) => serde_json::json!({"destination":s.destination()}),
            Target::Local { command, .. } => {
                serde_json::json!({"local":{"cwd":command.cwd.as_ref().map(|p|p.display().to_string()),"profile":self.settings.text("localShell"),"command":if self.settings.text("localShell")=="custom" {Some(self.settings.text("localShellCommand"))}else{None::<&str>}}})
            }
        };
        let (local, server_id) = match &target {
            Target::Ssh(s) => (false, Some(s.id.clone())),
            _ => (true, None),
        };
        let size = size(ui);
        let terminal = Arc::new(Mutex::new(Terminal::with_scrollback(
            size.rows as u16,
            size.cols as u16,
            self.settings.number("scrollback") as usize,
        )));
        let token = self.next_token;
        self.next_token += 1;
        let sink = Arc::new(Sink {
            token,
            terminal: Arc::clone(&terminal),
            events: self.sender.clone(),
        });
        let _runtime = self.runtime.enter();
        let (id, title) = match target {
            Target::Ssh(server) => {
                let title = server.name.clone();
                (self.core.sessions.open(server, size, sink), title)
            }
            Target::Local { command, title } => {
                (self.core.sessions.open_local(command, size, sink), title)
            }
        };
        self.tabs.push(Tab {
            token,
            id,
            title,
            local,
            server_id,
            terminal,
            status: SessionStatus::Connecting,
            message: "Подключение…".into(),
            prompts: VecDeque::new(),
            saved_target,
            pending: None,
        });
        self.select(ui, self.tabs.len() - 1);
        if self.settings.boolean("sidebarAutoHide") {
            ui.set_sidebar_hidden(true);
        }
        ui.set_notice("".into());
    }

    fn select(&mut self, ui: &AppWindow, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        // A paste belongs to its original session. Changing tabs cancels that confirmation.
        self.dialog = None;
        prompts::clear(ui);
        self.active = Some(index);
        ui.invoke_close_overlay();
        self.tabs[index].terminal.lock().unwrap().dirty = true;
        self.tabs_dirty = true;
    }

    fn close(&mut self, ui: &AppWindow, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        self.files.edits.stop_session(tab.id);
        self.core.sessions.close(tab.id);
        self.dialog = None;
        prompts::clear(ui);
        self.active = if self.tabs.is_empty() || self.active.is_none() {
            None
        } else {
            Some(match self.active.unwrap_or(0) {
                active if active > index => active - 1,
                active => active.min(self.tabs.len() - 1),
            })
        };
        if let Some(tab) = self.active() {
            tab.terminal.lock().unwrap().dirty = true;
        }
        if self.active.is_none() && self.settings.boolean("sidebarAutoHide") {
            ui.set_sidebar_hidden(self.settings.boolean("sidebarHidden"));
        }
        self.tabs_dirty = true;
    }

    fn tick(&mut self, ui: &AppWindow) {
        self.server_editor_events(ui);
        self.service_events(ui);
        self.agent_events(ui);
        self.file_events(ui);
        self.settings.capture(ui);
        let saved = self.settings.flush_if_due();
        if matches!(saved, Ok(true)) {
            let _entered = self.runtime.enter();
            self.agents.controller.apply(&self.settings.value);
        }
        report(ui, saved);
        // Limit event work per frame so status chatter cannot starve interaction.
        for _ in 0..128 {
            let Ok((token, event)) = self.events.try_recv() else {
                break;
            };
            let Some(tab) = self.tabs.iter_mut().find(|t| t.token == token) else {
                continue;
            };
            match event {
                SessionEvent::Status {
                    status, message, ..
                } => {
                    tab.status = status;
                    tab.message = message.unwrap_or_else(|| status_text(status).into());
                    self.tabs_dirty = true;
                }
                SessionEvent::Prompt { id, prompt } => tab.prompts.push_back((id, prompt)),
                SessionEvent::PromptClosed { id } => {
                    tab.prompts.retain(|(p, _)| *p != id);
                    if matches!(self.dialog, Some(Dialog::Auth { id: current, .. }) if current == id)
                    {
                        self.dialog = None;
                        prompts::clear(ui);
                    }
                }
                SessionEvent::Log {
                    level: nexssh_core::LogLevel::Error,
                    message,
                } => {
                    tab.message = message;
                    self.tabs_dirty = true;
                }
                _ => {}
            }
        }
        // Replies such as ConPTY's initial cursor query must also work in background tabs.
        for tab in &self.tabs {
            let replies = tab.terminal.lock().unwrap().replies();
            if !replies.is_empty() {
                let _ = self.core.sessions.write(tab.id, replies);
            }
        }
        if self.tabs_dirty {
            if self.settings.text("restoreTabs") == "always" {
                self.snapshot_tabs(true);
            }
            let rows: Vec<_> = self
                .tabs
                .iter()
                .map(|tab| TabRow {
                    title: tab.title.clone().into(),
                    status: status_text(tab.status).into(),
                    state: state_code(tab.status),
                    local: tab.local,
                })
                .collect();
            ui.set_tabs(ModelRc::new(VecModel::from(rows)));
            ui.set_active_tab(self.active.map_or(-1, |i| i as i32));
            ui.set_active_state(self.active().map_or(0, |t| state_code(t.status)));
            ui.set_active_local(self.active().is_some_and(|t| t.local));
            ui.set_selected_server(
                self.active()
                    .and_then(|t| t.server_id.as_deref())
                    .unwrap_or("")
                    .into(),
            );
            ui.set_status(
                self.active()
                    .map_or("Готово", |t| t.message.as_str())
                    .into(),
            );
            self.tabs_dirty = false;
            self.servers(ui);
        }
        let Some(tab) = self.active() else {
            ui.set_cells(ModelRc::default());
            ui.set_cursor_visible(false);
            ui.set_scrollback(0);
            return;
        };
        let mut terminal = tab.terminal.lock().unwrap();
        terminal.set_theme(ui.get_resolved_theme());
        terminal.highlighting = self.settings.boolean("highlightKeywords");
        terminal.highlight_colors = crate::highlight::KINDS.map(|kind| {
            let value = self.settings.text(&format!("highlightColors.{kind}"));
            let value = u32::from_str_radix(value.trim_start_matches('#'), 16).unwrap_or(0);
            slint::Color::from_rgb_u8((value >> 16) as u8, (value >> 8) as u8, value as u8)
        });
        let size = size(ui);
        if terminal.resize(size.rows as u16, size.cols as u16) {
            let _ = self.core.sessions.resize(tab.id, size);
        }
        if terminal.dirty {
            ui.set_cells(ModelRc::new(VecModel::from(terminal.frame())));
            let screen = terminal.parser.screen();
            let (row, col) = screen.cursor_position();
            ui.set_cursor_row(row.into());
            ui.set_cursor_col(col.into());
            ui.set_cursor_character(
                screen
                    .cell(row, col)
                    .map_or("", |cell| cell.contents())
                    .into(),
            );
            ui.set_cursor_visible(!screen.hide_cursor() && screen.scrollback() == 0);
            ui.set_scrollback(screen.scrollback() as i32);
        }
        drop(terminal);
        if self.dialog.is_none()
            && let Some((id, prompt)) = tab.prompts.front()
        {
            prompts::show(ui, prompt);
            self.dialog = Some(Dialog::Auth {
                token: tab.token,
                id: *id,
            });
        }
    }

    fn write(&self, ui: &AppWindow, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(tab) = self.active() {
            tab.terminal.lock().unwrap().follow_output();
            report(ui, self.core.sessions.write(tab.id, bytes));
        }
    }

    fn key(&mut self, ui: &AppWindow, text: &str, ctrl: bool, alt: bool, shift: bool) {
        if self.dialog.is_some() {
            return;
        }
        if ctrl && text == "\t" && !self.tabs.is_empty() {
            let step = if shift { self.tabs.len() - 1 } else { 1 };
            self.select(ui, (self.active.unwrap_or(0) + step) % self.tabs.len());
            return;
        }
        if ctrl && shift && text.eq_ignore_ascii_case("w") {
            if let Some(index) = self.active {
                self.close(ui, index);
            }
            return;
        }
        if ctrl && shift && text.eq_ignore_ascii_case("r") {
            self.reconnect_active(ui);
            return;
        }
        if ctrl && shift && text.eq_ignore_ascii_case("b") {
            ui.set_sidebar_hidden(!ui.get_sidebar_hidden());
            return;
        }
        if ctrl && shift && ["`", "~"].contains(&text) {
            report(ui, self.open(ui, None));
            return;
        }
        if ctrl
            && !alt
            && !shift
            && let Ok(index) = text.parse::<usize>()
            && (1..=9).contains(&index)
        {
            self.select(ui, index - 1);
            return;
        }
        if text == char::from(slint::platform::Key::F11).to_string() {
            ui.window().set_fullscreen(!ui.window().is_fullscreen());
            return;
        }
        if ctrl && !alt && ["=", "+", "-", "0"].contains(&text) {
            ui.set_font_size(if text == "0" {
                13.0
            } else {
                (ui.get_font_size() + if text == "-" { -1.0 } else { 1.0 }).clamp(9.0, 28.0)
            });
            return;
        }
        if text == "\n"
            && self
                .active()
                .is_some_and(|t| t.status == SessionStatus::Disconnected)
        {
            self.reconnect_active(ui);
            return;
        }
        if ctrl && shift && text.eq_ignore_ascii_case("v") {
            self.paste(ui);
            return;
        }
        if ctrl && shift && text.eq_ignore_ascii_case("c") {
            self.copy(ui);
            return;
        }
        if shift && text == char::from(slint::platform::Key::PageUp).to_string() {
            self.scroll(15);
            return;
        }
        if shift && text == char::from(slint::platform::Key::PageDown).to_string() {
            self.scroll(-15);
            return;
        }
        let application = self.active().is_some_and(|t| {
            t.terminal
                .lock()
                .unwrap()
                .parser
                .screen()
                .application_cursor()
        });
        self.write(
            ui,
            terminal::encode_key(text, ctrl, alt, shift, application),
        );
    }

    fn scroll(&self, lines: i32) {
        if let Some(tab) = self.active() {
            tab.terminal.lock().unwrap().scroll(lines);
        }
    }

    fn write_clipboard(&self, ui: &AppWindow, text: &str, message_key: &str) {
        if text.is_empty() {
            return;
        }
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text)) {
            Ok(()) => {
                let message = i18n::tr(message_key, i18n::resolve(self.settings.text("language")))
                    .replace("{text}", text);
                ui.invoke_show_copy_notice(message.into());
            }
            Err(error) => ui.set_notice(error.to_string().into()),
        }
    }

    fn copy(&self, ui: &AppWindow) {
        let Some(tab) = self.active() else { return };
        let terminal = tab.terminal.lock().unwrap();
        let selected = terminal.selected();
        let contents = if selected.is_empty() {
            terminal.parser.screen().contents()
        } else {
            selected
        };
        drop(terminal);
        self.write_clipboard(ui, &contents, "agents.copied");
    }

    fn paste(&mut self, ui: &AppWindow) {
        if self.dialog.is_some() {
            return;
        }
        let Some(tab) = self.active() else { return };
        let text = match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(text) => text,
            Err(error) => {
                ui.set_notice(error.to_string().into());
                return;
            }
        };
        if text.len() > 1024 * 1024 {
            ui.set_notice("Слишком большой текст для вставки (больше 1 МиБ)".into());
            return;
        }
        let bracketed = tab
            .terminal
            .lock()
            .unwrap()
            .parser
            .screen()
            .bracketed_paste();
        if text.contains(['\n', '\r'])
            && (self.settings.text("pasteWarning") == "always"
                || (self.settings.text("pasteWarning") == "auto" && !bracketed))
        {
            ui.set_prompt_title("Вставить несколько строк?".into());
            ui.set_prompt_detail(
                format!(
                    "Текст может выполнить команды в терминале.\n\n{}",
                    text.chars().take(800).collect::<String>()
                )
                .into(),
            );
            ui.set_prompt_fields(ModelRc::default());
            ui.set_prompt_host_key(false);
            ui.set_prompt_can_accept(true);
            ui.set_prompt_visible(true);
            self.dialog = Some(Dialog::Paste {
                session: tab.id,
                text,
            });
        } else {
            let bytes = tab.terminal.lock().unwrap().paste(&text);
            self.write(ui, bytes);
        }
    }

    fn answer(&mut self, ui: &AppWindow, accept: bool) {
        match self.dialog.take() {
            Some(Dialog::Auth { token, id }) => {
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.token == token)
                    && let Some(index) = tab.prompts.iter().position(|(p, _)| *p == id)
                {
                    let (_, prompt) = tab.prompts.remove(index).unwrap();
                    report(
                        ui,
                        self.core
                            .sessions
                            .answer(id, prompts::reply(ui, &prompt, accept)),
                    );
                }
            }
            Some(Dialog::Paste { session, text }) if accept => {
                if let Some(tab) = self.tabs.iter().find(|t| t.id == session) {
                    let mut terminal = tab.terminal.lock().unwrap();
                    let bytes = terminal.paste(&text);
                    terminal.follow_output();
                    report(ui, self.core.sessions.write(session, bytes));
                }
            }
            Some(Dialog::Restore { backup }) if accept => {
                let password = ui
                    .get_prompt_fields()
                    .row_data(0)
                    .map(|f| f.value.to_string())
                    .unwrap_or_default();
                self.services.prepare_restore(backup, password);
            }
            Some(Dialog::Agent { id }) => self.agents.controller.answer(id, accept, false),
            Some(Dialog::RenewToken) if accept => {
                let agents = Arc::clone(&self.agents.controller);
                let core = Arc::clone(&self.core);
                self.services.job(move || {
                    agents.renew_token_blocking(&core.secrets);
                    Ok(Some(ServiceEvent::Notice("Token updated".into())))
                });
            }
            Some(Dialog::RestoreTabs) if accept => self.restore_tabs(ui),
            Some(Dialog::InstallUpdate) if accept => {
                self.snapshot_tabs(true);
                if let Err(error) = self
                    .settings
                    .flush()
                    .map_err(|e| e.to_string())
                    .and_then(|_| self.services.updater.install().map_err(|e| e.to_string()))
                {
                    ui.set_notice(error.into());
                } else {
                    let _ = slint::quit_event_loop();
                }
            }
            Some(Dialog::DeleteServer { id, name }) if accept => {
                if ui.get_overlay() == "server" && ui.get_server_form().id == id {
                    self.server_editor.cancel(ui);
                    ui.invoke_close_overlay();
                }
                self.server_editor.delete(Arc::clone(&self.core), id, name);
            }
            _ => {}
        }
        prompts::clear(ui);
        ui.invoke_focus_terminal();
    }
}

fn matches_server(row: &ServerRow, filter: &str) -> bool {
    format!("{} {} {}", row.name, row.address, row.group)
        .to_lowercase()
        .contains(filter)
}

fn format_size(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else {
        size.to_string()
    }
}

fn status_text(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Connecting => "Подключение…",
        SessionStatus::Connected => "Подключено",
        SessionStatus::Disconnected => "Соединение завершено",
        SessionStatus::Closed => "Закрыто",
    }
}

fn state_code(status: SessionStatus) -> i32 {
    match status {
        SessionStatus::Connecting => 1,
        SessionStatus::Connected => 2,
        _ => 0,
    }
}

fn size(ui: &AppWindow) -> PtySize {
    PtySize::new(
        ui.get_terminal_cols().clamp(2, 400) as u32,
        ui.get_terminal_rows().clamp(2, 150) as u32,
    )
}

fn report<T, E: std::fmt::Display>(ui: &AppWindow, result: Result<T, E>) {
    if let Err(error) = result {
        ui.set_notice(error.to_string().into());
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = nexssh_core::default_data_dir().ok_or("Не найдена папка данных NexSSH")?;
    crate::logger::init();
    let settings = Settings::load(&data_dir)?;
    // The inspector installs its own headless backend when the first window is made.
    if std::env::var("SLINT_BACKEND").as_deref() != Ok("headless") {
        let mut backend = slint::BackendSelector::new()
            .with_winit_custom_application_handler(crate::keyboard::Shortcuts::default());
        #[cfg(windows)]
        {
            backend = backend.with_winit_window_attributes_hook(|attributes| {
                use slint::winit_030::winit::platform::windows::{
                    CornerPreference, WindowAttributesExtWindows,
                };
                // DWM rounds restored windows; maximized/full-screen corners stay square.
                attributes
                    .with_undecorated_shadow(true)
                    .with_corner_preference(CornerPreference::Round)
            });
        }
        if std::env::var_os("SLINT_BACKEND").is_none() {
            backend = backend.backend_name("winit".into());
            if !settings.boolean("gpuAcceleration") {
                backend = backend.renderer_name("software".into());
            }
        }
        backend.select()?;
    }
    let ui = AppWindow::new()?;
    i18n::init(&ui, settings.text("language"));
    crate::colors::init(&ui);
    settings.apply(&ui);
    if let Some(path) = &settings.recovery {
        ui.set_notice(format!("Повреждённые настройки сохранены: {path}").into());
    }
    let collapsed = settings.value["collapsedGroups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect();
    let core = Arc::new(Core::open(data_dir)?);
    let services = services::Services::new(Arc::clone(&core));
    let (sender, events) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let agents = crate::agents_ui::AgentUi::new(Arc::clone(&core), &runtime, &settings.value);
    let app = Rc::new(RefCell::new(App {
        startup_cwd: std::env::args_os()
            .skip_while(|arg| arg != "--cwd")
            .nth(1)
            .map(Into::into),
        files: crate::files::Files::new(&core),
        core,
        settings,
        services,
        agents,
        runtime,
        sender,
        events,
        tabs: vec![],
        active: None,
        next_token: 1,
        filter: String::new(),
        collapsed,
        dialog: None,
        tabs_dirty: true,
        server_editor: crate::server_editor::Editor::default(),
    }));
    app.borrow().servers(&ui);
    {
        let mut state = app.borrow_mut();
        if state.startup_cwd.is_some() {
            report(&ui, state.open(&ui, None));
        }
        let enabled = state.settings.boolean("explorerMenu");
        state.services.job(move || {
            crate::explorer::apply(enabled)?;
            Ok(None)
        });
    }

    if app.borrow().settings.boolean("autoUpdateCheck") {
        app.borrow_mut().services.check_updates();
    }
    // Each callback owns a weak UI handle so closing the window releases its component tree.
    macro_rules! bind {
        ($callback:ident, |$state:ident, $window:ident $(, $arg:ident)*| $body:block) => {{
            let state = Rc::clone(&app);
            let weak = ui.as_weak();
            ui.$callback(move |$($arg),*| {
                if let Some($window) = weak.upgrade() {
                    let $state = &mut *state.borrow_mut();
                    $body
                }
            });
        }};
    }
    bind!(on_terminal_find, |state, window, query, previous| {
        if let Some(tab) = state.active()
            && !tab.terminal.lock().unwrap().find(&query, previous)
        {
            window.set_notice("Не найдено / No matches".into());
        }
    });
    bind!(on_file_action, |state, window, key, path| {
        state.file_action(&window, &key, &path);
    });
    bind!(on_files_show_hidden, |state, window, show| {
        state.patch_setting(
            &window,
            "filesShowHidden",
            if show { "true" } else { "false" },
        );
    });
    bind!(on_settings_save_color, |state, window, color, save| {
        let mut saved = state.settings.value["savedColors"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        saved.retain(|c| c.as_str() != Some(color.as_str()));
        if save && crate::settings::valid_color(&color) {
            if saved.len() >= 14 {
                saved.remove(0);
            }
            saved.push(color.to_string().into());
        }
        state.settings.set_value("savedColors", saved.into());
        state.refresh_saved_colors(&window);
    });
    bind!(on_open_settings, |state, window| {
        state.refresh_settings(&window);
        window.set_overlay("settings".into());
    });
    bind!(on_settings_select, |state, window, section| {
        if settings_schema::SECTIONS.contains(&section.as_str()) {
            window.set_settings_section(section);
            state.refresh_settings(&window);
        }
    });
    bind!(on_settings_update, |state, window, key, value| {
        state.patch_setting(&window, &key, &value);
    });
    bind!(on_settings_action, |state, window, key, value| {
        state.setting_action(&window, &key, &value);
    });
    bind!(
        on_settings_association,
        |state, window, index, extensions, editor, command| {
            state.association(&window, index, &extensions, &editor, &command);
        }
    );
    bind!(on_open_local, |state, window| {
        report(&window, state.open(&window, None));
    });
    bind!(on_connect_server, |state, window, id| {
        if let Some(server) = state.core.store.get(&id) {
            state.connect_server(&window, server, false);
        }
    });
    bind!(on_quick_connect, |state, window, destination| {
        if let Some(server) = state.core.store.find(&destination) {
            report(&window, state.open(&window, Some(server)));
            return;
        }
        match Destination::parse(&destination) {
            Ok(d) => report(&window, state.open(&window, Some(d.to_server()))),
            Err(error) => window.set_notice(error.to_string().into()),
        }
    });
    bind!(on_filter_servers, |state, window, filter| {
        state.filter = filter.to_string();
        state.servers(&window);
    });
    bind!(on_palette_query, |_state, window, query| {
        let query = query.to_lowercase();
        let rows: Vec<_> = window
            .get_servers()
            .iter()
            .filter(|row| matches_server(row, &query))
            .collect();
        window.set_palette_servers(ModelRc::new(VecModel::from(rows)));
    });
    bind!(on_refresh_servers, |state, window| {
        state.servers(&window);
    });
    bind!(on_toggle_group, |state, window, group| {
        if !state.collapsed.remove(group.as_str()) {
            state.collapsed.insert(group.to_string());
        }
        state
            .settings
            .set_value("collapsedGroups", serde_json::json!(state.collapsed));
        state.servers(&window);
    });
    bind!(on_show_home, |state, window| {
        state.active = None;
        if state.settings.boolean("sidebarAutoHide") {
            window.set_sidebar_hidden(state.settings.boolean("sidebarHidden"));
        }
        state.tabs_dirty = true;
        state.dialog = None;
        prompts::clear(&window);
        window.invoke_close_overlay();
    });
    bind!(on_save_server, |state, window, connect| {
        state.server_editor.save(
            &window,
            Arc::clone(&state.core),
            connect,
            i18n::resolve(state.settings.text("language")),
        );
    });
    bind!(on_server_action, |state, window, action, id| {
        state.server_action(&window, &action, &id);
    });
    bind!(on_import_servers, |state, window| {
        match state.core.import_ssh_config(None, "") {
            Ok(report) => {
                state.servers(&window);
                window.set_notice(
                    format!(
                        "Импортировано: {}, обновлено: {}",
                        report.summary.added, report.summary.updated
                    )
                    .into(),
                );
            }
            Err(error) => window.set_notice(error.to_string().into()),
        }
    });
    bind!(on_dismiss_notice, |_state, window| {
        window.set_notice("".into());
    });
    bind!(on_close_window, |state, window| {
        state.request_close(&window);
    });
    bind!(on_close_decide, |state,
                            window,
                            action,
                            remember,
                            reopen| {
        state.decide_close(&window, &action, remember, reopen);
    });
    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            ui.invoke_close_window();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    bind!(on_select_tab, |state, window, index| {
        state.select(&window, index as usize);
    });
    bind!(on_close_tab, |state, window, index| {
        state.close(&window, index as usize);
    });
    bind!(on_reconnect, |state, window| {
        state.reconnect_active(&window);
    });
    bind!(on_terminal_key, |state, window, text, ctrl, alt, shift| {
        state.key(&window, &text, ctrl, alt, shift);
    });
    bind!(on_scroll_terminal, |state, _window, lines| {
        state.scroll(lines);
    });
    bind!(on_copy_screen, |state, window| {
        state.copy(&window);
    });
    bind!(on_paste, |state, window| {
        state.paste(&window);
    });
    bind!(on_terminal_select, |state, window, row, col, phase| {
        if let Some(tab) = state.active() {
            tab.terminal.lock().unwrap().select(row, col, phase == 0);
        }
        if phase == 2
            && state.settings.boolean("copyOnSelect")
            && state
                .active()
                .is_some_and(|t| !t.terminal.lock().unwrap().selected().is_empty())
        {
            state.copy(&window);
        }
    });
    bind!(on_terminal_context, |state, window| {
        if state.settings.boolean("rightClickPaste") {
            if state
                .active()
                .is_some_and(|t| !t.terminal.lock().unwrap().selected().is_empty())
            {
                state.copy(&window);
            } else {
                state.paste(&window);
            }
        } else {
            window.set_overlay("menu".into());
        }
    });
    bind!(on_answer_prompt, |state, window, accept| {
        state.answer(&window, accept);
    });

    let tray = if std::env::var("SLINT_BACKEND").is_ok_and(|s| s == "headless") {
        None
    } else {
        crate::Tray::new().ok()
    };
    if let Some(tray) = &tray {
        ui.set_tray_available(true);
        let weak = ui.as_weak();
        tray.on_open(move || {
            if let Some(ui) = weak.upgrade() {
                let _ = ui.show();
                ui.window().set_minimized(false);
            }
        });
        let state = Rc::clone(&app);
        let weak = ui.as_weak();
        tray.on_quit(move || {
            if let Some(ui) = weak.upgrade() {
                let mut state = state.borrow_mut();
                let reopen = state.settings.text("restoreTabs") == "always";
                state.decide_close(&ui, "quit", false, reopen);
            }
        });
        let lang = i18n::resolve(app.borrow().settings.text("language"));
        tray.set_open_label(i18n::tr("tray.open", lang).into());
        tray.set_quit_label(i18n::tr("close.quit", lang).into());
    }
    {
        let mut app = app.borrow_mut();
        if app.settings.value["lastTabs"]["tabs"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            match app.settings.text("restoreTabs") {
                "always" => app.restore_tabs(&ui),
                "ask" => {
                    let lang = i18n::resolve(app.settings.text("language"));
                    ui.set_prompt_title(i18n::tr("settings.restoreTabs", lang).into());
                    ui.set_prompt_detail(
                        app.settings.value["lastTabs"]["tabs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter_map(|t| t["title"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                            .into(),
                    );
                    ui.set_prompt_fields(ModelRc::default());
                    ui.set_prompt_auth(false);
                    ui.set_prompt_host_key(false);
                    ui.set_prompt_can_accept(true);
                    ui.set_prompt_visible(true);
                    app.dialog = Some(Dialog::RestoreTabs);
                }
                _ => {}
            }
        }
    }
    let timer = Timer::default();
    let weak = ui.as_weak();
    let tick_app = Rc::clone(&app);
    timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
        if let Some(ui) = weak.upgrade() {
            tick_app.borrow_mut().tick(&ui);
        }
    });
    let result = ui.run();
    timer.stop();
    let mut app = app.borrow_mut();
    app.files.cancel();
    app.files.edits.stop_all();
    app.agents
        .controller
        .apply(&serde_json::json!({"agentsEnabled":false}));
    app.core.sessions.close_all();
    // Let the core release local child processes and connections before dropping its runtime.
    app.runtime.block_on(async {
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            while app.core.sessions.session_count() > 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
    });
    app.settings.capture(&ui);
    app.settings.flush()?;
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use std::time::Instant;

    #[test]
    fn restored_ssh_tabs_are_dormant_and_keep_the_original_server_target() {
        let (ui, mut app, _dir) = setup();
        let server = app
            .core
            .store
            .save_server(Server {
                name: "Restore test".into(),
                host: "example.invalid".into(),
                ..Server::default()
            })
            .unwrap();
        app.settings.set_value("lastTabs",serde_json::json!({"active":0,"tabs":[{"kind":"ssh","target":{"serverId":server.id},"title":"Restore test"}]}));
        app.restore_tabs(&ui);
        app.tick(&ui);
        assert_eq!(app.core.sessions.session_count(), 0);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tabs[0].pending.is_some());
        assert_eq!(ui.get_active_tab(), 0);
        assert_eq!(ui.get_active_state(), 0);
        app.snapshot_tabs(true);
        assert_eq!(
            app.settings.value["lastTabs"]["tabs"][0]["target"]["serverId"],
            server.id
        );
        app.close(&ui, 0);
        assert_eq!(app.core.sessions.session_count(), 0);
    }

    #[test]
    fn file_preferences_control_hidden_entries_and_keep_folders_first_when_descending() {
        let (ui, mut app, _dir) = setup();
        let entry = |name: &str, kind, size| nexssh_core::sftp::Entry {
            name: name.into(),
            kind,
            link_to_dir: false,
            size,
            modified: None,
            permissions: None,
            mode: None,
        };
        use nexssh_core::sftp::EntryKind::{Dir, File};
        app.files.path = "/home/test".into();
        app.files.entries = vec![
            entry(".env", File, 1),
            entry("small", File, 10),
            entry("large", File, 1000),
            entry("directory", Dir, 0),
        ];
        app.patch_setting(&ui, "filesSort", "size");
        app.patch_setting(&ui, "filesSortDesc", "true");
        let names = ui
            .get_file_entries()
            .iter()
            .map(|r| r.name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, ["directory", "large", "small"]);
        app.patch_setting(&ui, "filesShowHidden", "true");
        assert_eq!(ui.get_file_entries().row_count(), 4);
        app.settings.flush().unwrap();
        let saved = Settings::load(&app.core.data_dir).unwrap();
        assert!(saved.boolean("filesShowHidden"));
        assert!(saved.boolean("filesSortDesc"));
        assert_eq!(saved.text("filesSort"), "size");
    }

    fn setup() -> (AppWindow, App, tempfile::TempDir) {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.show().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1160.0, 740.0));
        let dir = tempfile::tempdir().unwrap();
        let core =
            Core::with_secrets(dir.path(), nexssh_core::secrets::Secrets::in_memory()).unwrap();
        let (sender, events) = mpsc::channel();
        i18n::init(&ui, "ru");
        crate::colors::init(&ui);
        let core = Arc::new(core);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let agents = crate::agents_ui::AgentUi::new(
            Arc::clone(&core),
            &runtime,
            &serde_json::json!({"agentsEnabled":false}),
        );
        let app = App {
            agents,
            files: crate::files::Files::new(&core),
            startup_cwd: None,
            settings: Settings::memory(dir.path()),
            services: services::Services::new(Arc::clone(&core)),
            core,
            runtime,
            sender,
            events,
            tabs: vec![],
            active: None,
            next_token: 1,
            filter: String::new(),
            collapsed: HashSet::new(),
            dialog: None,
            tabs_dirty: true,
            server_editor: crate::server_editor::Editor::default(),
        };
        (ui, app, dir)
    }

    fn local_target() -> Target {
        #[cfg(windows)]
        let command = nexssh_core::LocalCommand {
            program: "cmd.exe".into(),
            args: vec!["/d".into(), "/q".into()],
            env: vec![],
            cwd: None,
        };
        #[cfg(not(windows))]
        let command = nexssh_core::LocalCommand {
            program: "/bin/sh".into(),
            args: vec![],
            env: vec![("TERM".into(), "xterm-256color".into())],
            cwd: None,
        };
        Target::Local {
            command,
            title: "Test shell".into(),
        }
    }

    #[test]
    fn local_session_sends_input_renders_output_resizes_and_closes() {
        let (ui, mut app, _dir) = setup();
        app.start(&ui, local_target());
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut typed = false;
        loop {
            app.tick(&ui);
            let tab = app.active().unwrap();
            let screen = tab.terminal.lock().unwrap().parser.screen().contents();
            if !typed && tab.status == SessionStatus::Connected && !screen.is_empty() {
                #[cfg(windows)]
                let line = b"set /a 12345*3\r".to_vec();
                #[cfg(not(windows))]
                let line = b"echo $((12345*3))\r".to_vec();
                app.write(&ui, line);
                typed = true;
            }
            if screen.contains("37035") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "No command result; screen={screen:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(ui.get_tabs().row_count(), 1);
        assert!(ui.get_cells().row_count() > 0);
        ui.window().set_size(slint::LogicalSize::new(1000.0, 600.0));
        app.tick(&ui);
        let screen_size = app
            .active()
            .unwrap()
            .terminal
            .lock()
            .unwrap()
            .parser
            .screen()
            .size();
        assert_eq!(screen_size.1.get() as i32, ui.get_terminal_cols());
        app.close(&ui, 0);
        app.tick(&ui);
        assert_eq!(ui.get_active_tab(), -1);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.core.sessions.session_count() > 0 {
            assert!(Instant::now() < deadline, "Local shell was not released");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn filters_saved_servers_by_group_and_address() {
        let (ui, mut app, _dir) = setup();
        app.core
            .store
            .save_server(Server {
                name: "Production".into(),
                host: "example.org".into(),
                group: "Infrastructure".into(),
                ..Server::default()
            })
            .unwrap();
        app.filter = "infrastructure".into();
        app.servers(&ui);
        assert_eq!(ui.get_servers().row_count(), 1);
        app.filter = "missing".into();
        app.servers(&ui);
        assert_eq!(ui.get_grouped_servers().row_count(), 0);
        assert_eq!(ui.get_servers().row_count(), 1);
        assert_eq!(ui.get_home_servers().row_count(), 2);
        app.filter.clear();
        app.collapsed.insert("Infrastructure".into());
        app.servers(&ui);
        assert!(ui.get_grouped_servers().row_data(0).unwrap().collapsed);
        assert!(ui.get_grouped_servers().row_data(1).unwrap().hidden);
        assert!(!ui.get_home_servers().row_data(1).unwrap().hidden);
    }

    #[test]
    fn background_auth_waits_for_its_tab_and_closing_it_clears_secret_fields() {
        let (ui, mut app, _dir) = setup();
        for token in 1..=2 {
            app.tabs.push(Tab {
                token,
                id: token,
                title: format!("Tab {token}"),
                local: false,
                server_id: None,
                status: SessionStatus::Connecting,
                message: String::new(),
                terminal: Arc::new(Mutex::new(Terminal::new(24, 80))),
                prompts: VecDeque::new(),
                saved_target: serde_json::Value::Null,
                pending: None,
            });
        }
        app.select(&ui, 1);
        app.sender
            .send((
                1,
                SessionEvent::Prompt {
                    id: 7,
                    prompt: Prompt::Password {
                        user: "test".into(),
                        host: "example.org".into(),
                        can_remember: false,
                        error: None,
                    },
                },
            ))
            .unwrap();
        app.tick(&ui);
        assert!(!ui.get_prompt_visible());
        app.select(&ui, 0);
        app.tick(&ui);
        assert!(ui.get_prompt_visible());
        assert!(ui.get_prompt_fields().row_data(0).unwrap().secret);
        app.close(&ui, 0);
        app.tick(&ui);
        assert!(!ui.get_prompt_visible());
        assert_eq!(ui.get_prompt_fields().row_count(), 0);
        assert_eq!(app.active().unwrap().token, 2);
    }
}
