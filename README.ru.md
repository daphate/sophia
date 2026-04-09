# Sophia Bot

Telegram-юзербот на базе Claude CLI. Написан на Rust.

Возможности: постоянная память, пейринг пользователей, выполнение команд ОС, история диалогов.

## Требования

- [Rust](https://rustup.rs/) (stable, 1.75+)
- Учётные данные Telegram API (`api_id`, `api_hash` с https://my.telegram.org)
- [Claude CLI](https://docs.anthropic.com/en/docs/claude-code) установлен и авторизован (`claude login`)

## Установка

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
PHONE_NUMBER=+1234567890
OWNER_ID=your_telegram_id
CLAUDE_CLI=claude
INFERENCE_TIMEOUT=120
SESSION_NAME=sophia
EXEC_ENABLED=true
EXEC_ALLOWED_COMMANDS=cat,echo,ls,pwd,date,whoami,uname,head,tail,wc,df,free,uptime,tee
```

| Переменная | Описание |
|---|---|
| `API_ID` | Telegram API ID с https://my.telegram.org |
| `API_HASH` | Telegram API hash |
| `PHONE_NUMBER` | Номер телефона Telegram-аккаунта, от имени которого работает бот |
| `OWNER_ID` | Ваш Telegram ID (числовой). Бот считает этого пользователя администратором |
| `CLAUDE_CLI` | Путь к Claude CLI (по умолчанию: `claude`) |
| `INFERENCE_TIMEOUT` | Макс. время ожидания ответа Claude в секундах (по умолчанию: `120`) |
| `SESSION_NAME` | Имя файла сессии Telegram (по умолчанию: `sophia`) |
| `EXEC_ENABLED` | Включить команду `/exec` (по умолчанию: `true`) |
| `EXEC_ALLOWED_COMMANDS` | Белый список разрешённых команд ОС через запятую |

### 3. Первый запуск

```bash
./target/release/sophia
```

При первом запуске потребуется ввести:
1. Код авторизации Telegram (придёт в приложение Telegram)
2. Пароль 2FA (если включён)

После авторизации сессия сохраняется и повторный ввод не потребуется.

### 4. Режим отладки

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

Создайте `~/Library/LaunchAgents/com.sophia.bot.plist`:

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

```bash
launchctl load ~/Library/LaunchAgents/com.sophia.bot.plist
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
| `/help` | Спаренные | Показать справку |

## Архитектура

```
src/
  main.rs        — Точка входа, авторизация, цикл обновлений, graceful shutdown
  config.rs      — Конфигурация, загрузка .env, константы путей
  handlers.rs    — Диспетчер команд, обработка сообщений
  inference.rs   — Подпроцесс Claude CLI, парсинг JSON
  memory.rs      — Память, диалоги, генерация системного промпта
  pairing.rs     — Спаренные/ожидающие пользователи (оба persistent)
  queue.rs       — SQLite очередь сообщений
  telegram.rs    — Реакции, отправка длинных сообщений, скачивание медиа

data/
  instructions/  — Файлы системного промпта (см. ниже)
  memory/        — Рантайм-память (авто через [MEMORY_UPDATE] теги)
  dialogs/       — Логи диалогов по пользователям и дням
  users/         — Данные пейринга (paired.json, pending.json, owner.json)
  files/         — Скачанные медиафайлы
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

## Лицензия

MIT
