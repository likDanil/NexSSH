//! Messages the core shows to people: connection progress, errors, prompt hints.
//!
//! Every user-facing text lives in this file with all its translations, so a new
//! language is added in one place (and the compiler points at every message that
//! misses it). Log lines for developers (`log::debug!` etc.) stay in English.
//!
//! The language is process-wide, like a C locale: English by default, set by the
//! application from its settings with [`set_language`].

use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

/// A supported interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    Ru,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];

    /// Parses a language tag such as `ru`, `ru-RU` or `en_US.UTF-8`.
    pub fn from_tag(tag: &str) -> Option<Lang> {
        let base = tag.trim().split(['-', '_', '.']).next()?;
        Lang::ALL
            .into_iter()
            .find(|l| l.tag().eq_ignore_ascii_case(base))
    }

    /// The BCP 47 tag (`en`, `ru`).
    pub fn tag(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

/// Sets the language of messages produced from now on.
pub fn set_language(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn language() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Ru,
        _ => Lang::En,
    }
}

/// Declares message functions returning the text in the current language.
/// Placeholders are the function's arguments (`{host}`), checked at compile time.
macro_rules! messages {
    ($(
        $(#[$meta:meta])*
        fn $name:ident($($arg:ident: $ty:ty),* $(,)?) {
            en: $en:literal,
            ru: $ru:literal $(,)?
        }
    )*) => {
        $(
            $(#[$meta])*
            #[allow(clippy::useless_format)]
            pub fn $name($($arg: $ty),*) -> String {
                match language() {
                    Lang::En => format!($en),
                    Lang::Ru => format!($ru),
                }
            }
        )*
    };
}

messages! {
    // ---- connecting ------------------------------------------------------------------

    fn connecting(dest: &str) {
        en: "Connecting to {dest}…",
        ru: "Подключение к {dest}…",
    }
    fn connecting_through_jump(dest: &str) {
        en: "Connecting to {dest} through the jump host…",
        ru: "Подключение к {dest} через jump-хост…",
    }
    fn connecting_to_jump(dest: &str) {
        en: "Connecting to jump host {dest}…",
        ru: "Подключение к jump-хосту {dest}…",
    }
    fn jump_host_failed(dest: &str, error: impl Display) {
        en: "jump host {dest}: {error}",
        ru: "jump-хост {dest}: {error}",
    }
    fn jump_host_unreachable(target: &str, error: impl Display) {
        en: "the jump host could not reach {target}: {error}",
        ru: "jump-хост не смог подключиться к {target}: {error}",
    }
    fn jump_chain_too_long() {
        en: "the jump host chain is too long",
        ru: "слишком длинная цепочка jump-хостов",
    }
    fn jump_chain_loop() {
        en: "the jump host chain is too long (do two servers use each other as jump host?)",
        ru: "слишком длинная цепочка jump-хостов (возможно, два сервера указаны jump-хостами друг для друга)",
    }
    fn cannot_resolve(host: &str) {
        en: "cannot resolve {host}",
        ru: "не удалось найти адрес {host}",
    }
    fn cannot_resolve_because(host: &str, error: impl Display) {
        en: "cannot resolve {host}: {error}",
        ru: "не удалось найти адрес {host}: {error}",
    }
    fn cannot_connect(target: &str, error: impl Display) {
        en: "cannot connect to {target}: {error}",
        ru: "не удалось подключиться к {target}: {error}",
    }
    fn attempt_timed_out() {
        en: "timed out",
        ru: "время ожидания истекло",
    }
    fn timed_out_after(secs: u32) {
        en: "connection timed out after {secs}s",
        ru: "время подключения истекло ({secs} с)",
    }

    // ---- SSH protocol ----------------------------------------------------------------

    fn ssh_timeout() {
        en: "connection timed out",
        ru: "время подключения истекло",
    }
    fn ssh_keepalive_timeout() {
        en: "server stopped responding (keepalive timeout)",
        ru: "сервер перестал отвечать (keepalive)",
    }
    fn ssh_inactivity() {
        en: "connection closed after inactivity",
        ru: "соединение закрыто из-за неактивности",
    }
    fn ssh_unknown_key() {
        en: "server host key was not accepted",
        ru: "ключ хоста не принят",
    }
    fn ssh_disconnected() {
        en: "disconnected",
        ru: "отключено",
    }
    fn ssh_hangup() {
        en: "connection closed by remote host",
        ru: "соединение закрыто удалённым хостом",
    }
    fn ssh_no_common_algorithm(detail: impl Display) {
        en: "no algorithm in common with the server ({detail})",
        ru: "нет общих алгоритмов с сервером ({detail})",
    }
    fn closed_by_server(message: &str) {
        en: "closed by server: {message}",
        ru: "закрыто сервером: {message}",
    }
    fn closed_by_server_code(code: impl Display) {
        en: "closed by server ({code})",
        ru: "закрыто сервером ({code})",
    }

    // ---- host keys -------------------------------------------------------------------

    fn host_key_failed(detail: &str) {
        en: "host key verification failed: {detail}",
        ru: "проверка ключа хоста не пройдена: {detail}",
    }
    fn host_key_revoked(host: &str) {
        en: "the key presented by {host} is marked as revoked",
        ru: "ключ, предъявленный {host}, отозван",
    }
    fn host_key_changed_rejected() {
        en: "the host key has changed and was not accepted",
        ru: "ключ хоста изменился и не был принят",
    }
    fn host_key_rejected() {
        en: "the host key was not accepted",
        ru: "ключ хоста не принят",
    }
    fn host_key_save_failed(error: impl Display) {
        en: "Could not save host key: {error}",
        ru: "Не удалось сохранить ключ хоста: {error}",
    }
    fn host_key_changed_accepted(host: &str, fingerprint: &str) {
        en: "Accepted a changed host key for {host} ({fingerprint})",
        ru: "Принят изменённый ключ хоста {host} ({fingerprint})",
    }

    // ---- authentication --------------------------------------------------------------

    fn auth_failed(detail: &str) {
        en: "authentication failed ({detail})",
        ru: "не удалось войти ({detail})",
    }
    fn auth_no_method(offered: &str) {
        en: "no usable method; server offers: {offered}",
        ru: "нет подходящего способа входа; сервер предлагает: {offered}",
    }
    fn auth_nothing_offered() {
        en: "nothing",
        ru: "ничего",
    }
    fn auth_tried(methods: &str) {
        en: "tried {methods}",
        ru: "испробовано: {methods}",
    }
    fn authenticated(user: &str, method: &str) {
        en: "Authenticated as {user} ({method})",
        ru: "Вход выполнен как {user} ({method})",
    }
    fn authenticated_without_auth() {
        en: "Authenticated (no authentication required)",
        ru: "Вход выполнен (аутентификация не требуется)",
    }
    fn auth_partial(method: &str) {
        en: "{method} accepted, the server requires another method",
        ru: "{method}: принято, сервер требует ещё один способ входа",
    }
    fn method_password() {
        en: "password",
        ru: "пароль",
    }
    fn method_keyboard_interactive() {
        en: "keyboard-interactive",
        ru: "keyboard-interactive",
    }
    fn method_key(path: &str) {
        en: "key {path}",
        ru: "ключ {path}",
    }
    fn method_agent_key(name: impl Display) {
        en: "agent key {name}",
        ru: "ключ из агента {name}",
    }
    fn method_agent_certificate(name: &str) {
        en: "agent certificate {name}",
        ru: "сертификат из агента {name}",
    }
    fn agent_no_response() {
        en: "SSH agent did not respond",
        ru: "SSH-агент не ответил",
    }
    fn agent_sign_failed(error: impl Display) {
        en: "SSH agent could not sign: {error}",
        ru: "SSH-агент не смог подписать запрос: {error}",
    }
    fn wrong_password() {
        en: "Permission denied, please try again.",
        ru: "Доступ запрещён, попробуйте ещё раз.",
    }
    fn wrong_passphrase() {
        en: "Incorrect passphrase",
        ru: "Неверная парольная фраза",
    }
    fn password_saved() {
        en: "Password saved to the system keychain",
        ru: "Пароль сохранён в системном хранилище",
    }
    fn password_save_failed(error: impl Display) {
        en: "Could not save password: {error}",
        ru: "Не удалось сохранить пароль: {error}",
    }
    fn passphrase_save_failed(error: impl Display) {
        en: "Could not save passphrase: {error}",
        ru: "Не удалось сохранить парольную фразу: {error}",
    }

    // ---- keys ------------------------------------------------------------------------

    fn key_unreadable(path: &str, error: impl Display) {
        en: "cannot read key {path}: {error}",
        ru: "не удалось прочитать ключ {path}: {error}",
    }
    fn key_invalid(path: &str, error: impl Display) {
        en: "invalid key {path}: {error}",
        ru: "неверный ключ {path}: {error}",
    }
    fn key_unsupported(path: &str, error: impl Display) {
        en: "unsupported key {path}: {error}",
        ru: "неподдерживаемый ключ {path}: {error}",
    }
    fn key_error(error: impl Display) {
        en: "key error: {error}",
        ru: "ошибка ключа: {error}",
    }
    /// Key type shown when only the (encrypted) private half is known.
    fn private_key() {
        en: "private key",
        ru: "закрытый ключ",
    }

    // ---- sessions --------------------------------------------------------------------

    fn disconnected() {
        en: "Disconnected",
        ru: "Отключено",
    }
    fn auth_cancelled() {
        en: "Authentication cancelled",
        ru: "Вход отменён",
    }
    fn not_connected() {
        en: "not connected",
        ru: "нет подключения",
    }
    fn session_ended_early() {
        en: "session ended",
        ru: "сеанс завершён",
    }
    fn shell_failed(error: impl Display) {
        en: "could not start a shell: {error}",
        ru: "не удалось запустить командную оболочку: {error}",
    }
    fn exit_code(code: u32) {
        en: "exit code {code}",
        ru: "код выхода {code}",
    }
    fn exit_signal(signal: impl Display) {
        en: "signal {signal}",
        ru: "сигнал {signal}",
    }
    fn session_ended(how: &str) {
        en: "Session ended ({how})",
        ru: "Сеанс завершён ({how})",
    }
    fn connection_lost(reason: &str) {
        en: "Connection lost: {reason}",
        ru: "Соединение потеряно: {reason}",
    }
    fn connection_closed() {
        en: "Connection closed",
        ru: "Соединение закрыто",
    }
    fn cancelled() {
        en: "cancelled",
        ru: "отменено",
    }
    fn session_not_found(id: u64) {
        en: "session {id} not found",
        ru: "сеанс {id} не найден",
    }
    fn prompt_not_found() {
        en: "this question was already answered",
        ru: "на этот запрос уже ответили",
    }

    // ---- port forwarding -------------------------------------------------------------

    fn forwarding(description: &str) {
        en: "Forwarding {description}",
        ru: "Проброс {description}",
    }
    fn forwarding_failed(description: &str, error: impl Display) {
        en: "Port forwarding {description}: {error}",
        ru: "Проброс портов {description}: {error}",
    }
    fn cannot_listen(bind: &str, error: impl Display) {
        en: "cannot listen on {bind}: {error}",
        ru: "не удалось открыть порт {bind}: {error}",
    }
    fn server_refused_listen(bind: &str, error: impl Display) {
        en: "the server refused to listen on {bind}: {error}",
        ru: "сервер отказался открыть порт {bind}: {error}",
    }
    fn forward_local(bind: &str, target: &str) {
        en: "{bind} → {target}",
        ru: "{bind} → {target}",
    }
    fn forward_remote(bind: &str, target: &str) {
        en: "remote {bind} → {target}",
        ru: "на сервере {bind} → {target}",
    }
    fn forward_dynamic(bind: &str) {
        en: "SOCKS {bind}",
        ru: "SOCKS {bind}",
    }
    fn forward_needs_local_port() {
        en: "Forwarding needs a local port",
        ru: "Для проброса нужен локальный порт",
    }
    fn forward_needs_target_host() {
        en: "Forwarding needs a destination host",
        ru: "Для проброса нужен хост назначения",
    }
    fn forward_needs_target_port() {
        en: "Forwarding needs a destination port",
        ru: "Для проброса нужен порт назначения",
    }

    // ---- servers ---------------------------------------------------------------------

    fn host_required() {
        en: "Host is required",
        ru: "Укажите хост",
    }
    fn host_has_spaces() {
        en: "Host must not contain spaces",
        ru: "Хост не может содержать пробелы",
    }
    fn port_out_of_range() {
        en: "Port must be between 1 and 65535",
        ru: "Порт должен быть от 1 до 65535",
    }
    fn user_has_spaces() {
        en: "Username must not contain spaces",
        ru: "Имя пользователя не может содержать пробелы",
    }
    fn key_file_required() {
        en: "Choose a private key file for key authentication",
        ru: "Выберите файл закрытого ключа для входа по ключу",
    }
    fn destination_required() {
        en: "Enter a host, e.g. user@example.com:22",
        ru: "Введите хост, например user@example.com:22",
    }
    fn destination_has_spaces() {
        en: "Destination must not contain spaces",
        ru: "Адрес не может содержать пробелы",
    }
    fn ipv6_missing_bracket() {
        en: "Missing ']' in IPv6 address",
        ru: "В IPv6-адресе не хватает ']'",
    }
    fn ipv6_trailing_text() {
        en: "Unexpected text after ']'",
        ru: "Лишний текст после ']'",
    }
    fn invalid_port(port: &str) {
        en: "Invalid port '{port}'",
        ru: "Неверный порт «{port}»",
    }
    fn group_name_empty() {
        en: "Group name must not be empty",
        ru: "Название группы не может быть пустым",
    }
    fn server_missing() {
        en: "This server no longer exists",
        ru: "Этого сервера больше нет",
    }
    fn nothing_to_connect() {
        en: "Nothing to connect to",
        ru: "Не указано, куда подключаться",
    }
    fn password_not_stored(error: impl Display) {
        en: "Saved, but the password could not be stored ({error}). You will be asked for it when connecting.",
        ru: "Сервер сохранён, но пароль сохранить не удалось ({error}). Его спросят при подключении.",
    }

    // ---- files (SFTP) ----------------------------------------------------------------

    fn sftp_unavailable(error: impl Display) {
        en: "SFTP is not available on this server: {error}",
        ru: "SFTP на этом сервере недоступен: {error}",
    }
    fn sftp_no_such_file(path: &str) {
        en: "{path}: no such file or folder",
        ru: "{path}: файл или папка не найдены",
    }
    fn sftp_permission_denied(path: &str) {
        en: "{path}: permission denied",
        ru: "{path}: нет доступа",
    }
    fn sftp_exists(path: &str) {
        en: "{path} already exists",
        ru: "{path} уже существует",
    }
    fn sftp_failed(path: &str, error: impl Display) {
        en: "{path}: {error}",
        ru: "{path}: {error}",
    }
    fn local_write_failed(path: &str, error: impl Display) {
        en: "cannot write {path}: {error}",
        ru: "не удалось записать {path}: {error}",
    }
    fn local_read_failed(path: &str, error: impl Display) {
        en: "cannot read {path}: {error}",
        ru: "не удалось прочитать {path}: {error}",
    }
    fn reveal_failed(error: impl Display) {
        en: "could not open the file manager: {error}",
        ru: "не удалось открыть файловый менеджер: {error}",
    }

    // ---- application updates ---------------------------------------------------------

    fn update_check_failed(error: impl Display) {
        en: "could not check for updates: {error}",
        ru: "не удалось проверить обновления: {error}",
    }
    fn update_download_failed(error: impl Display) {
        en: "could not download the update: {error}",
        ru: "не удалось скачать обновление: {error}",
    }
    fn update_install_failed(error: impl Display) {
        en: "could not install the update: {error}",
        ru: "не удалось установить обновление: {error}",
    }
    fn update_missing() {
        en: "there is no update to download; check for updates first",
        ru: "нет обновления для загрузки; сначала проверьте обновления",
    }
    fn update_not_installable() {
        en: "this copy of NexSSH cannot install updates itself; download the new version from the releases page",
        ru: "эта копия NexSSH не может обновиться сама; скачайте новую версию со страницы релизов",
    }
    fn update_not_downloaded() {
        en: "the update has not been downloaded yet",
        ru: "обновление ещё не скачано",
    }

    // ---- files and storage -----------------------------------------------------------

    fn not_found(what: &str) {
        en: "{what} not found",
        ru: "{what}: не найдено",
    }
    fn home_not_found() {
        en: "home directory not found",
        ru: "не найдена домашняя папка",
    }
    fn invalid_data(error: impl Display) {
        en: "invalid data: {error}",
        ru: "неверные данные: {error}",
    }
    fn keychain_error(error: impl Display) {
        en: "system keychain: {error}",
        ru: "системное хранилище паролей: {error}",
    }
    fn proxy_command_unsupported(alias: &str) {
        en: "{alias}: ProxyCommand is not supported",
        ru: "{alias}: ProxyCommand не поддерживается",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_language_tags() {
        assert_eq!(Lang::from_tag("ru"), Some(Lang::Ru));
        assert_eq!(Lang::from_tag("ru-RU"), Some(Lang::Ru));
        assert_eq!(Lang::from_tag("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::from_tag("EN"), Some(Lang::En));
        assert_eq!(Lang::from_tag("de-DE"), None);
        assert_eq!(Lang::from_tag(""), None);
    }

    #[test]
    fn messages_follow_the_language() {
        // Other unit tests do not compare message texts, so switching briefly is safe.
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_language(Lang::En);
            }
        }
        let _reset = Reset;
        assert_eq!(connecting("web-01"), "Connecting to web-01…");
        set_language(Lang::Ru);
        assert_eq!(language(), Lang::Ru);
        assert_eq!(connecting("web-01"), "Подключение к web-01…");
        assert_eq!(timed_out_after(15), "время подключения истекло (15 с)");
        set_language(Lang::En);
        assert_eq!(timed_out_after(15), "connection timed out after 15s");
    }
}
