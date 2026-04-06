import asyncio
import logging

from telethon import TelegramClient

from bot.config import Config, PROJECT_ROOT
from bot.handlers import register_handlers
from bot.pairing import save_owner

logger = logging.getLogger(__name__)


async def run():
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    )

    config = Config.from_env()
    session_path = str(PROJECT_ROOT / config.session_name)

    client = TelegramClient(session_path, config.api_id, config.api_hash)

    logger.info("Starting Sophia userbot...")
    await client.start(phone=config.phone_number)

    me = await client.get_me()
    logger.info("Logged in as %s %s (ID: %d)", me.first_name, me.last_name or "", me.id)

    # Save owner info
    save_owner({
        "id": config.owner_id,
        "bot_user_id": me.id,
        "bot_name": f"{me.first_name} {me.last_name or ''}".strip(),
    })

    register_handlers(client, config, me.id)
    logger.info("Handlers registered. Sophia is running.")

    await client.run_until_disconnected()
