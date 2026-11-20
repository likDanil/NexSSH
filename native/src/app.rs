use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use nexssh_core::{
    Core, Destination, EventSink, Prompt, PtySize, Server, SessionEvent, SessionId, SessionStatus,
};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

use crate::{
    AppWindow, ServerRow, TabRow, prompts,
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
    status: SessionStatus,
    message: String,
    terminal: SharedTerminal,
    prompts: VecDeque<(u64, Prompt)>,
}

enum Target {
    Ssh(Server),
    Local {
        command: nexssh_core::LocalCommand,
        title: String,
    },
}

enum Dialog {
    Auth { token: u64, id: u64 },
    Paste { session: SessionId, text: String },
}

struct App {
    core: Core,
    runtime: tokio::runtime::Runtime,
    events: mpsc::Receiver<(u64, SessionEvent)>,
    sender: mpsc::Sender<(u64, SessionEvent)>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    next_token: u64,
    filter: String,
    dialog: Option<Dialog>,
    tabs_dirty: bool,
}

impl App {
    fn servers(&self, ui: &AppWindow) {
        let filter = self.filter.to_lowercase();
        let rows: Vec<ServerRow> = self
            .core
            .store
            .snapshot()
            .servers
            .into_iter()
            .filter(|s| {
                format!("{} {} {} {}", s.name, s.host, s.user, s.group)
                    .to_lowercase()
                    .contains(&filter)
            })
            .map(|s| ServerRow {
                id: s.id.clone().into(),
                name: s.name.clone().into(),
                address: s.destination().into(),
                group: s.group.into(),
            })
            .collect();
        ui.set_servers(ModelRc::new(VecModel::from(rows)));
    }

    fn active(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    fn open(
        &mut self,
        ui: &AppWindow,
        server: Option<Server>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let target = if let Some(server) = server {
            Target::Ssh(server)
        } else {
            let profile = nexssh_core::local::shells()
                .into_iter()
                .next()
                .ok_or("Не найдена локальная оболочка")?;
            Target::Local {
                command: profile.command(None),
                title: profile.name,
            }
        };
        self.start(ui, target);
        Ok(())
    }

    fn start(&mut self, ui: &AppWindow, target: Target) {
        let size = size(ui);
        let terminal = Arc::new(Mutex::new(Terminal::new(
            size.rows as u16,
            size.cols as u16,
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
            terminal,
            status: SessionStatus::Connecting,
            message: "Подключение…".into(),
            prompts: VecDeque::new(),
        });
        self.select(ui, self.tabs.len() - 1);
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
        self.tabs[index].terminal.lock().unwrap().dirty = true;
        self.tabs_dirty = true;
    }

    fn close(&mut self, ui: &AppWindow, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(index);
        self.core.sessions.close(tab.id);
        self.dialog = None;
        prompts::clear(ui);
        self.active = if self.tabs.is_empty() {
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
        self.tabs_dirty = true;
    }

    fn tick(&mut self, ui: &AppWindow) {
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
            let rows: Vec<_> = self
                .tabs
                .iter()
                .map(|tab| TabRow {
                    title: tab.title.clone().into(),
                    status: status_text(tab.status).into(),
                })
                .collect();
            ui.set_tabs(ModelRc::new(VecModel::from(rows)));
            ui.set_active_tab(self.active.map_or(-1, |i| i as i32));
            ui.set_status(
                self.active()
                    .map_or("Готово", |t| t.message.as_str())
                    .into(),
            );
            self.tabs_dirty = false;
        }
        let Some(tab) = self.active() else {
            ui.set_cells(ModelRc::default());
            ui.set_cursor_visible(false);
            ui.set_scrollback(0);
            return;
        };
        let mut terminal = tab.terminal.lock().unwrap();
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

    fn copy(&self, ui: &AppWindow) {
        let Some(tab) = self.active() else { return };
        let contents = tab.terminal.lock().unwrap().parser.screen().contents();
        report(
            ui,
            arboard::Clipboard::new().and_then(|mut c| c.set_text(contents)),
        );
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
        if text.contains(['\n', '\r']) {
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
            _ => {}
        }
        prompts::clear(ui);
        ui.invoke_focus_terminal();
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
    let ui = AppWindow::new()?;
    if !cfg!(windows) {
        ui.set_terminal_font("monospace".into());
    }
    let data_dir = nexssh_core::default_data_dir().ok_or("Не найдена папка данных NexSSH")?;
    let (sender, events) = mpsc::channel();
    let app = Rc::new(RefCell::new(App {
        core: Core::open(data_dir)?,
        runtime: tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
        sender,
        events,
        tabs: vec![],
        active: None,
        next_token: 1,
        filter: String::new(),
        dialog: None,
        tabs_dirty: true,
    }));
    app.borrow().servers(&ui);

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
    bind!(on_open_local, |state, window| {
        report(&window, state.open(&window, None));
    });
    bind!(on_connect_server, |state, window, id| {
        if let Some(server) = state.core.store.get(&id) {
            report(&window, state.open(&window, Some(server)));
        }
    });
    bind!(on_quick_connect, |state, window, destination| {
        match Destination::parse(&destination) {
            Ok(d) => report(&window, state.open(&window, Some(d.to_server()))),
            Err(error) => window.set_notice(error.to_string().into()),
        }
    });
    bind!(on_filter_servers, |state, window, filter| {
        state.filter = filter.to_string();
        state.servers(&window);
    });
    bind!(on_refresh_servers, |state, window| {
        state.servers(&window);
    });
    bind!(on_select_tab, |state, window, index| {
        state.select(&window, index as usize);
    });
    bind!(on_close_tab, |state, window, index| {
        state.close(&window, index as usize);
    });
    bind!(on_reconnect, |state, window| {
        if let Some(tab) = state.active() {
            report(&window, state.core.sessions.reconnect(tab.id));
        }
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
    bind!(on_answer_prompt, |state, window, accept| {
        state.answer(&window, accept);
    });

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
    let app = app.borrow();
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
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use std::time::Instant;

    fn setup() -> (AppWindow, App, tempfile::TempDir) {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        ui.show().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1160.0, 740.0));
        let dir = tempfile::tempdir().unwrap();
        let core =
            Core::with_secrets(dir.path(), nexssh_core::secrets::Secrets::in_memory()).unwrap();
        let (sender, events) = mpsc::channel();
        let app = App {
            core,
            runtime: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
            sender,
            events,
            tabs: vec![],
            active: None,
            next_token: 1,
            filter: String::new(),
            dialog: None,
            tabs_dirty: true,
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
        assert_eq!(ui.get_servers().row_count(), 0);
    }

    #[test]
    fn background_auth_waits_for_its_tab_and_closing_it_clears_secret_fields() {
        let (ui, mut app, _dir) = setup();
        for token in 1..=2 {
            app.tabs.push(Tab {
                token,
                id: token,
                title: format!("Tab {token}"),
                status: SessionStatus::Connecting,
                message: String::new(),
                terminal: Arc::new(Mutex::new(Terminal::new(24, 80))),
                prompts: VecDeque::new(),
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
