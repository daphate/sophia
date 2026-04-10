# Sophia Bot v1.0-beta

> Дата релиза: 2026-04-09

Telegram-бот на базе Claude CLI. Написан на Rust. Два бинарника (основной + rescue), использующие общую библиотеку.

Возможности: постоянная память, пейринг пользователей, выполнение команд ОС, история диалогов по пользователям, семантический поиск, автоматическая проверка обновлений.

## Быстрая установка

**Linux / macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/daphate/sophia/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/daphate/sophia/main/install.ps1 | iex
```

### Требования

- [Rust](https://rustup.rs/) (stable, 1.75+)
- Учётные данные Telegram API (`api_id`, `api_hash` с https://my.telegram.org)
- [Claude CLI](https://docs.anthropic.com/en/docs/claude-code) установлен и авторизован (`claude login`)

## Ручная установка

### 1. Клонирование и сборка

```bash
git clone https://github.com/daphate/sophia.git
cd sophia
cargo build --release
```

### 2. Настройка

```bash
cp .env.example .env
```

Отредактируйте `.env`:

```env
API_ID=your_api_id
API_HASH=your_api_hash
# BOT_TOKEN для обычного режима, или PHONE_NUMBER для юзербота
BOT_TOKEN=123456:ABC-DEF
# PHONE_NUMBER=+1234567890
OWNER_ID=your_telegram_id
CLAUDE_CLI=claude
INFERENCE_TIMEOUT=120
SESSION_NAME=sophia
EXEC_ENABLED=true
EXEC_ALLOWED_COMMANDS=cat,echo,ls,pwd,date,whoami,uname,head,tail,wc,df,free,uptime,tee
UPDATE_CHECK_HOURS=12
```

| Переменная | Описание |
|---|---|
| `API_ID` | Telegram API ID с https://my.telegram.org |
| `API_HASH` | Telegram API hash |
| `BOT_TOKEN` | Токен бота от [@BotFather](https://t.me/BotFather) (режим бота) |
| `PHONE_NUMBER` | Номер телефона (режим юзербота). Требуется либо `BOT_TOKEN`, либо `PHONE_NUMBER` |
| `OWNER_ID` | Ваш Telegram ID (числовой). Бот считает этого пользователя администратором |
| `CLAUDE_CLI` | Путь к Claude CLI (по умолчанию: `claude`) |
| `INFERENCE_TIMEOUT` | Макс. время ожидания ответа Claude в секундах (по умолчанию: `120`) |
| `SESSION_NAME` | Имя файла сессии Telegram (по умолчанию: `sophia`) |
| `EXEC_ENABLED` | Включить команду `/exec` (по умолчанию: `true`) |
| `EXEC_ALLOWED_COMMANDS` | Белый список разрешённых команд ОС через запятую |
| `UPDATE_CHECK_HOURS` | Интервал проверки обновлений в часах. `0` = отключено (по умолчанию: `12`) |
| `AUTO_UPDATE` | Автоматически обновлять, пересобирать и перезапускать (по умолчанию: `false`) |
| `RESCUE_BOT_TOKEN` | Токен бота-сторожа sophia-rescue (необязательно) |

### 3. Первый запуск

```bash
./target/release/sophia
```

При первом запуске потребуется ввести:
1. Код авторизации Telegram (придёт в приложение Telegram)
2. Пароль 2FA (если включён)

После авторизации сессия сохраняется и повторный ввод не потребуется.

### 4. Обновление

Sophia проверяет обновления автоматически (каждые 12 часов по умолчанию).

**Ручное обновление:**

```bash
cd sophia
git pull
cargo build --release
```

Затем перезапустите бота (см. ниже).

**Автоматическое обновление:** установите `AUTO_UPDATE=true` в `.env`. При обнаружении новой версии бот выполнит pull, пересборку и перезапуск (код выхода 42 запускает перезапуск через сервис-менеджер).

### 5. Перезапуск

**systemd (Linux):**

```bash
sudo systemctl restart sophia-bot
```

**launchd (macOS):**

```bash
# Перезапуск основного бота
launchctl kickstart -k gui/$(id -u)/com.sophia.bot
# Перезапуск rescue-бота
launchctl kickstart -k gui/$(id -u)/com.sophia.rescue
```

**Windows (NSSM):**

```powershell
nssm restart Sophia
```

**Без сервис-менеджера:** используйте обёртку:

```bash
./run.sh
```

### 6. Режим отладки

```bash
./target/release/sophia --debug
```

Выводит все необработанные обновления Telegram для диагностики.

## Установка по платформам

### Linux (systemd)

```bash
sudo cp sophia-bot.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sophia-bot
```

Отредактируйте `sophia-bot.service` под ваши пути:

```ini
[Service]
WorkingDirectory=/path/to/sophia
ExecStart=/path/to/sophia/target/release/sophia
User=your_user
```

### macOS (launchd)

Два launchd-сервиса: основной бот и бот-сторож.

**com.sophia.bot** — основной бот. Создайте `~/Library/LaunchAgents/com.sophia.bot.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.sophia.bot</string>
    <key>ProgramArguments</key>
    <array>
        <string>/path/to/sophia/target/release/sophia</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/path/to/sophia</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/sophia.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/sophia.err</string>
</dict>
</plist>
```

**com.sophia.rescue** — бот-сторож (см. раздел Rescue Bot ниже). Создайте `~/Library/LaunchAgents/com.sophia.rescue.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.sophia.rescue</string>
    <key>ProgramArguments</key>
    <array>
        <string>/path/to/sophia/target/release/sophia-rescue</string>
    </array>
    <key>WorkingDirectory</key>
    <string>/path/to/sophia</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/sophia-rescue.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/sophia-rescue.err</string>
</dict>
</plist>
```

```bash
launchctl load ~/Library/LaunchAgents/com.sophia.bot.plist
launchctl load ~/Library/LaunchAgents/com.sophia.rescue.plist
```

### Windows

Выполните в PowerShell:

```powershell
git clone https://github.com/daphate/sophia.git
cd sophia
cargo build --release
copy .env.example .env
# Отредактируйте .env
.\target\release\sophia.exe
```

Для работы в фоне используйте [NSSM](https://nssm.cc/):

```powershell
nssm install Sophia "C:\path\to\sophia\target\release\sophia.exe"
nssm set Sophia AppDirectory "C:\path\to\sophia"
nssm start Sophia
```

## Команды

Оба бота (основной и rescue) поддерживают одинаковый набор команд:

| Команда | Доступ | Описание |
|---|---|---|
| `/pair` | Все | Запросить доступ |
| `/approve <id>` | Владелец | Одобрить пейринг |
| `/deny <id>` | Владелец | Отклонить пейринг |
| `/unpair <id>` | Владелец | Удалить пользователя |
| `/exec <cmd>` | Владелец | Выполнить команду ОС |
| `/memory` | Владелец | Просмотр памяти |
| `/memory add <text>` | Владелец | Добавить в память |
| `/memory clear` | Владелец | Очистить память |
| `/update` | Владелец | Проверить и установить обновления |
| `/search <запрос>` | Владелец | Поиск по истории диалогов |
| `/reindex` | Владелец | Перестроить индекс семантического поиска |
| `/status` | Владелец | Проверить статус парного бота |
| `/restart` | Владелец | Перезапустить парного бота |
| `/logs` | Владелец | Показать последние логи |
| `/ping` | Спаренные | Проверка доступности с аптаймом |
| `/help` | Спаренные | Показать справку |

## Rescue Bot (sophia-rescue)

Бот-компаньон, использующий общую библиотеку (`src/lib.rs`) с основным ботом. Оба бинарника обладают полной функциональностью — разговоры с Claude, все команды. Разница в том, какой сервис каждый из них мониторит и перезапускает.

Rescue-бот работает как отдельный launchd/systemd-сервис и следит за основным ботом (и наоборот).

**Настройка:**

1. Создайте второго бота через [@BotFather](https://t.me/BotFather).
2. Установите `RESCUE_BOT_TOKEN` в `.env`.
3. Соберите: `cargo build --release` (собирает оба бинарника).
4. Установите команды в BotFather: `./scripts/set-commands.sh`
5. Настройте launchd-сервис (см. [macOS (launchd)](#macos-launchd)).

## Семантический поиск (Vector Store)

Sophia индексирует историю диалогов для семантического поиска. Используется [fastembed](https://github.com/Anush008/fastembed-rs) (модель `multilingual-e5-small`, 384 измерения) + [usearch](https://github.com/unum-cloud/usearch) для векторного хранилища.

Данные сохраняются в `data/vecstore.usearch`.

## Очередь сообщений (Message Queue)

SQLite-очередь (`queue.db`) для дедупликации входящих сообщений. Предотвращает повторную обработку при перезапусках.

## Исходящие сообщения (Outbox)

Проактивная отправка сообщений через файлы `data/outbox/*.json`. Скрипт `scripts/send.sh` создаёт файл сообщения, бот подхватывает и отправляет его автоматически.

## Архитектура

Cargo workspace с общей библиотекой (`src/lib.rs`) и двумя бинарниками (`src/main.rs` — основной бот, `sophia-rescue/` — rescue-бот). Оба бинарника линкуются с одной библиотекой, вся основная логика (обработчики, инференс, память, пейринг и т.д.) общая.

```
src/
  lib.rs          — Общая библиотека (реэкспортирует все модули ниже)
  main.rs         — Точка входа основного бота
  config.rs       — Конфигурация, загрузка .env, константы путей
  format.rs       — Конвертация Markdown → Telegram HTML + нарезка сообщений
  handlers.rs     — Диспетчер команд, обработка сообщений
  inference.rs    — Подпроцесс Claude CLI, парсинг JSON
  memory.rs       — Память, диалоги, генерация системного промпта
  outbox.rs       — Проактивная отправка сообщений через outbox
  pairing.rs      — Спаренные/ожидающие пользователи (оба persistent)
  queue.rs        — SQLite очередь сообщений с дедупликацией
  telegram.rs     — Реакции, отправка длинных сообщений, скачивание медиа
  update_check.rs — Периодическая проверка обновлений на GitHub
  vecstore.rs     — Векторное хранилище (fastembed + usearch)
  watchdog.rs     — Мониторинг парного бота (проверка launchd-сервиса)

sophia-rescue/src/
  main.rs         — Точка входа rescue-бота (использует общую библиотеку)

sophia-nexus/src/
  main.rs         — MCP-сервер для интеграции с Claude Code

data/
  instructions/   — Файлы системного промпта (см. ниже)
  memory/         — Рантайм-память (авто через [MEMORY_UPDATE] теги)
  dialogs/        — Логи диалогов по пользователям и дням
  users/          — Данные пейринга (paired.json, pending.json, owner.json)
  files/          — Скачанные медиафайлы
  outbox/         — JSON-файлы исходящих сообщений
```

### Файлы инструкций

Все файлы из `data/instructions/` загружаются в системный промпт:

| Файл | Назначение | В репо |
|---|---|---|
| `AGENTS.md` | Загрузка: правила старта, протокол памяти, красные линии | Да (шаблон) |
| `IDENTITY.md` | Личность бота: имя, роль, эмодзи, предыстория | Да (шаблон) |
| `SOUL.md` | Характер: стиль мышления, общение, границы | Да (шаблон) |
| `USER.md` | Информация о владельце: имя, часовой пояс, предпочтения | Да (шаблон) |
| `TOOLS.md` | Заметки об окружении (SSH, TTS, API) | Нет (gitignored) |
| `MEMORY.md` | Курированная долгосрочная память | Нет (gitignored) |

Скопируйте `.example` файлы для начала:

```bash
cp data/instructions/TOOLS.md.example data/instructions/TOOLS.md
cp data/instructions/MEMORY.md.example data/instructions/MEMORY.md
```

## Sophia NEXUS (MCP-сервер)

`sophia-nexus` — отдельный крейт в workspace, реализующий сервер [Model Context Protocol](https://modelcontextprotocol.io/). Даёт Claude Code (или любому MCP-совместимому клиенту) прямой доступ к данным Sophia: файлы личности, память, история диалогов, отправка сообщений в Telegram, семантический поиск.

Конфигурация в `.mcp.json` в корне проекта. Собирается вместе со всеми крейтами: `cargo build --release`.

## Лицензия

MIT
