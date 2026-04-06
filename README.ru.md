# Sophia v1.0-beta (2026-04-08)

Telegram-юзербот с бэкендом на Claude CLI + Anthropic-совместимый HTTP-прокси.

> **Дисклеймер:** Этот проект предоставляется исключительно в образовательных и исследовательских целях. Он демонстрирует, как теоретически можно построить Anthropic-совместимый прокси поверх Claude CLI. Авторы не призывают использовать это в продакшене или каким-либо образом, который может нарушать Условия использования Anthropic. Используйте на свой страх и риск.

## Зачем

Используйте [OpenClaw](https://openclaw.dev) (или любой Anthropic-совместимый клиент) с вашей собственной подпиской Claude Pro/Max — без API-ключей. Sophia Proxy оборачивает Claude CLI в стандартный эндпоинт `/v1/messages` (Anthropic Messages API), что позволяет развернуть собственный AI-стек на небольшом VPS или локально на Mac/PC, используя тот же Claude, за который вы уже платите.

## Компоненты

Репозиторий содержит два независимых компонента. Устанавливайте любой из них или оба:

| Компонент | Директория | Назначение |
|---|---|---|
| **Sophia Proxy** | `proxy/` | Anthropic-совместимый HTTP-прокси поверх Claude CLI |
| **Sophia Bot** | `bot/` | Telegram-юзербот с памятью, пейрингом, выполнением команд |

---

# Sophia Proxy

Транслирует вызовы Anthropic Messages API (`/v1/messages`) в вызовы Claude CLI. Поддерживает потоковый (SSE с инкрементальными дельтами) и обычный режимы. Передаёт изображения inline как base64. Stateless — без сессий, без файлов на диске.

## Требования

- [Rust](https://rustup.rs/) (stable)
- [Claude CLI](https://docs.anthropic.com/en/docs/claude-code) — установлен и авторизован (`claude login`)

## Быстрая установка

**Linux** (сборка + systemd-сервис):
```bash
curl -sSL https://raw.githubusercontent.com/daphate/sophia-proxy/main/proxy/install-linux.sh | bash
```

**macOS** (сборка + LaunchAgent):
```bash
curl -sSL https://raw.githubusercontent.com/daphate/sophia-proxy/main/proxy/install-mac.sh | bash
```

**Windows** (PowerShell, сборка + опциональный сервис):
```powershell
irm https://raw.githubusercontent.com/daphate/sophia-proxy/main/proxy/install-windows.ps1 | iex
```

## Ручная установка

```bash
git clone https://github.com/daphate/sophia-proxy.git
cd sophia-proxy/proxy
cargo build --release
cp .env.example .env   # отредактируйте
./target/release/sophia-proxy
```

Проверка:
```bash
curl http://127.0.0.1:8080/v1/models
curl http://127.0.0.1:8080/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-sophia-local" \
  -d '{"model":"claude-opus-4-6","max_tokens":1024,"messages":[{"role":"user","content":"Привет"}]}'
```

## Переменные окружения

| Переменная | По умолчанию | Описание |
|---|---|---|
| `PROXY_HOST` | `127.0.0.1` | Адрес привязки |
| `PROXY_PORT` | `8080` | Порт |
| `CLAUDE_CLI` | `claude` | Путь к Claude CLI |
| `MODEL_NAME` | `claude-opus-4-6` | Имя модели для клиентов |
| `INFERENCE_TIMEOUT` | _(нет)_ | Таймаут запроса в секундах |
| `MAX_TURNS` | _(нет)_ | Макс. количество ходов на запрос |
| `RUST_LOG` | `info` | Уровень логирования (`debug`, `info`, `error`) |

## Управление сервисом

**Linux (systemd):**
```bash
sudo systemctl start sophia-proxy
sudo systemctl stop sophia-proxy
sudo systemctl restart sophia-proxy
journalctl -u sophia-proxy -f
```

**macOS (LaunchAgent):**
```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.sophia.proxy.plist    # стоп
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.sophia.proxy.plist  # старт
tail -f /tmp/sophia-proxy.log
```

**Windows (Администратор):**
```powershell
sc.exe stop SophiaProxy
sc.exe start SophiaProxy
sc.exe delete SophiaProxy
```

## Интеграция с OpenClaw

Настройка [OpenClaw](https://openclaw.dev) для маршрутизации всех LLM-запросов через sophia-proxy с моделью `secondf8n/sophia`.

**Linux / macOS:**
```bash
cd sophia-proxy/proxy && ./setup-openclaw.sh
```

**Windows:**
```powershell
cd sophia-proxy\proxy; powershell -ExecutionPolicy Bypass -File setup-openclaw.ps1
```

Скрипт сделает резервную копию конфига, добавит `sophia-proxy` как провайдер и установит его основной моделью. Для ручной настройки добавьте в `~/.openclaw/openclaw.json`:

```json
{
  "models": {
    "providers": {
      "sophia-proxy": {
        "baseUrl": "http://127.0.0.1:8080/v1",
        "apiKey": "sk-sophia-local",
        "api": "anthropic-messages",
        "models": [
          {
            "id": "secondf8n/sophia",
            "name": "Sophia (Claude via CLI proxy)",
            "contextWindow": 1000000,
            "maxTokens": 64000
          }
        ]
      }
    }
  },
  "agents": {
    "defaults": {
      "model": {
        "primary": "sophia-proxy/secondf8n/sophia"
      }
    }
  }
}
```

Для удалённых серверов укажите `SOPHIA_PROXY_HOST` и `SOPHIA_PROXY_PORT` перед запуском скрипта.

---

# Sophia Bot (Telegram)

Telegram-юзербот на базе Claude CLI. Возможности: очередь сообщений с батчингом, постоянная память, пейринг пользователей, выполнение команд ОС.

## Требования

- Python 3.11+
- Учётные данные Telegram API (`api_id`, `api_hash` с https://my.telegram.org)
- Claude CLI установлен и авторизован

## Установка

```bash
git clone https://github.com/daphate/sophia-proxy.git
cd sophia-proxy
pip install -r requirements.txt

cat > .env <<'EOF'
API_ID=your_api_id
API_HASH=your_api_hash
PHONE_NUMBER=+1234567890
OWNER_ID=your_telegram_id
CLAUDE_CLI=claude
EOF

python3 start.py
```

**Systemd-сервис (Linux):**
```bash
sudo cp sophia-bot.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now sophia-bot
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
| `/queue` | Владелец | Статус очереди |
| `/help` | Спаренные | Показать справку |
