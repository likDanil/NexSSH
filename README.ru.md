<p align="center">
  <img src="desktop/icons/128x128.png" width="88" alt="Логотип NexSSH">
</p>

<h1 align="center">NexSSH</h1>

<p align="center">
  Быстрый минималистичный SSH-клиент, в котором главное — терминал.<br>
  <a href="https://github.com/likDanil/NexSSH/releases/latest"><b>Скачать для Windows</b></a> ·
  <a href="README.md">English</a> ·
  <a href="docs/ARCHITECTURE.md">Архитектура</a>
</p>

![NexSSH с подключённой сессией](docs/screenshots/main-light-ru.png)

## Зачем

* **Лёгкий.** Ядро на Rust и системный webview (Tauri 2) вместо встроенного браузера:
  маленький установщик, мало памяти, почти нулевая нагрузка на CPU в простое.
* **Спокойный интерфейс.** Только две области — серверы и терминал. Всё остальное —
  в командной палитре, меню и коротких диалогах.
* **Безопасный по умолчанию.** Пароли и passphrase хранятся только в системном хранилище
  (Windows Credential Manager, macOS Keychain, Secret Service); ключи хостов проверяются
  как в OpenSSH.

## Возможности

* Серверы с группами, поиском и статусом подключения; **импорт `~/.ssh/config`**
  (`Include`, шаблоны `Host`, `ProxyJump`, пробросы портов).
* Вкладки с настоящим терминалом (xterm.js, GPU-рендеринг), resize, копирование/вставка,
  переподключение клавишей <kbd>Enter</kbd>.
* Аутентификация: SSH-агент (OpenSSH agent, Pageant, 1Password…), ключи (OpenSSH, PEM,
  PKCS#8, PuTTY `.ppk`) — passphrase спрашивается, только если сервер принимает ключ —
  пароль и keyboard-interactive / 2FA.
* `known_hosts` (хэшированные записи, шаблоны) и понятное предупреждение при смене ключа хоста.
* Jump-хосты (в том числе цепочки), keepalive, таймауты подключения, нестандартные порты.
* Проброс портов: локальный (`-L`), удалённый (`-R`) и SOCKS5 (`-D`), с сохранением для сервера.
* Командная палитра и быстрое подключение: <kbd>Ctrl</kbd>/<kbd>⌘</kbd>+<kbd>K</kbd>, ввод `user@host:port`.
* Темы: светлая, серая, чёрная, синяя или как в системе.
* Языки: русский и английский (по умолчанию — как в системе; переключаются в настройках или в палитре).

![Командная палитра](docs/screenshots/command-palette.png)

## Темы

![Светлая, серая, чёрная и синяя темы](docs/screenshots/themes.png)

## Установка

**Windows 10/11:** скачайте `NexSSH_<версия>_x64-setup.exe` на странице
[Releases](https://github.com/likDanil/NexSSH/releases). Установка идёт для текущего
пользователя (без прав администратора); при необходимости установщик сам поставит WebView2.
Сборки пока не подписаны, поэтому SmartScreen может попросить подтверждение
(*Подробнее → Выполнить в любом случае*).

**macOS / Linux:** пока — сборка из исходников (см. ниже).

## Горячие клавиши

| Действие | Windows / Linux | macOS |
| --- | --- | --- |
| Командная палитра | <kbd>Ctrl+Shift+K</kbd> (<kbd>Ctrl+K</kbd> вне терминала) | <kbd>⌘K</kbd> |
| Новая сессия / быстрое подключение | <kbd>Ctrl+Shift+T</kbd> | <kbd>⌘T</kbd> |
| Закрыть вкладку | <kbd>Ctrl+Shift+W</kbd> | <kbd>⌘W</kbd> |
| Следующая / предыдущая вкладка | <kbd>Ctrl+Tab</kbd> / <kbd>Ctrl+Shift+Tab</kbd> | <kbd>⌘⇧]</kbd> / <kbd>⌘⇧[</kbd> |
| Вкладка 1–9 | <kbd>Ctrl+1…9</kbd> | <kbd>⌘1…9</kbd> |
| Переподключиться | <kbd>Enter</kbd> в закрытой сессии, <kbd>Ctrl+Shift+R</kbd> | <kbd>Enter</kbd>, <kbd>⌘R</kbd> |
| Копировать / вставить | <kbd>Ctrl+Shift+C</kbd> / <kbd>Ctrl+Shift+V</kbd> (<kbd>Ctrl+C</kbd> копирует выделение) | <kbd>⌘C</kbd> / <kbd>⌘V</kbd> |
| Скрыть/показать сайдбар | <kbd>Ctrl+Shift+B</kbd> | <kbd>⌘B</kbd> |
| Полный экран | <kbd>F11</kbd> | <kbd>⌃⌘F</kbd> |
| Настройки | <kbd>Ctrl+,</kbd> | <kbd>⌘,</kbd> |

В Windows и Linux обычные <kbd>Ctrl+K</kbd>, <kbd>Ctrl+W</kbd>, <kbd>Ctrl+R</kbd> и <kbd>Ctrl+B</kbd>
остаются за shell (удалить до конца строки / вырезать в nano, удалить слово, поиск по
истории, префикс tmux). В настройках можно разрешить <kbd>Ctrl+K</kbd> открывать палитру и
внутри терминала.

## Сборка из исходников

Нужны [Rust](https://rustup.rs) 1.89+, Node.js 20.19+ (или 22.12+) и
[зависимости Tauri](https://v2.tauri.app/start/prerequisites/) для вашей ОС
(Linux: `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libdbus-1-dev`).

```sh
npm install
npm run dev     # приложение с горячей перезагрузкой
npm run build   # оптимизированная сборка + установщик для текущей ОС
```

## Разработка

```
core/     SSH, аутентификация, known_hosts, проброс портов, хранилище (без GUI-зависимостей)
desktop/  оболочка Tauri: окно, IPC-команды, настройки
ui/       Svelte 5 + TypeScript + xterm.js
```

Подробно — в [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

Проверки (те же, что в CI):

```sh
npm run check
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Интеграционные тесты подключаются к настоящему OpenSSH. В Linux серверы запускаются так:

```sh
sudo scripts/test-sshd.sh
eval "$(sudo scripts/test-sshd.sh env)"
cargo test -p nexssh-core --test sshd
```

Переменные окружения: `NEXSSH_DATA_DIR` — другая папка данных (портативный режим, тесты),
`NEXSSH_LOG` — уровень логов, `NEXSSH_UI_OS` — предпросмотр заголовка окна другой ОС.

**Папка данных:** `%APPDATA%\NexSSH` (Windows), `~/Library/Application Support/NexSSH` (macOS),
`~/.config/nexssh` (Linux). Там лежат `servers.json`, `settings.json` и `known_hosts` —
секретов там нет никогда.

## Релизы

Релизы собирает GitHub Actions ([release.yml](.github/workflows/release.yml)):

* запушить тег — `git tag v0.2.0 && git push origin v0.2.0`, или
* **Actions → Release → Run workflow**: указать версию или оставить поле пустым —
  тогда выйдет следующая патч-версия. *Dry run* собирает установщик без публикации.

Workflow подставляет версию в `Cargo.toml`, собирает NSIS-установщик на Windows и публикует
релиз с `NexSSH_<версия>_x64-setup.exe` и `SHA256SUMS.txt`.
Пуши в ветки, которые меняют настройки релиза (workflow, `tauri.conf.json`, иконки), запускают
его в режиме dry run: установщик собирается и прикрепляется к запуску workflow, но не публикуется.

## Планы

* SFTP-панель (загрузка/скачивание, drag & drop)
* Разделение терминала (split)
* Сниппеты и история команд
* Установщики для macOS и Linux, автообновление, подпись кода
* Синхронизация настроек

## Лицензия

[MIT](LICENSE)
