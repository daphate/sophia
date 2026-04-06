import os
from dataclasses import dataclass, field
from pathlib import Path
from dotenv import load_dotenv

load_dotenv()

PROJECT_ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = PROJECT_ROOT / "data"
INSTRUCTIONS_DIR = DATA_DIR / "instructions"
MEMORY_DIR = DATA_DIR / "memory"
DIALOGS_DIR = DATA_DIR / "dialogs"
USERS_DIR = DATA_DIR / "users"
FILES_DIR = DATA_DIR / "files"

OWNER_FILE = USERS_DIR / "owner.json"
PAIRED_FILE = USERS_DIR / "paired.json"
MEMORY_FILE = MEMORY_DIR / "MEMORY.md"

AGENTS_FILE = INSTRUCTIONS_DIR / "AGENTS.md"
SOUL_FILE = INSTRUCTIONS_DIR / "SOUL.md"
USER_FILE = INSTRUCTIONS_DIR / "USER.md"


@dataclass
class Config:
    api_id: int = 0
    api_hash: str = ""
    phone_number: str = ""
    owner_id: int = 0
    claude_cli: str = "claude"
    inference_timeout: int = 150
    session_name: str = "sophia"
    exec_enabled: bool = True
    exec_allowed_commands: list[str] = field(default_factory=list)

    @classmethod
    def from_env(cls) -> "Config":
        api_id = os.getenv("API_ID", "")
        api_hash = os.getenv("API_HASH", "")
        phone_number = os.getenv("PHONE_NUMBER", "")
        owner_id = os.getenv("OWNER_ID", "")

        if not all([api_id, api_hash, phone_number, owner_id]):
            raise ValueError(
                "Missing required .env variables: API_ID, API_HASH, PHONE_NUMBER, OWNER_ID"
            )

        allowed = os.getenv(
            "EXEC_ALLOWED_COMMANDS",
            "cat,echo,ls,pwd,date,whoami,uname,head,tail,wc,df,free,uptime,tee",
        )

        return cls(
            api_id=int(api_id),
            api_hash=api_hash,
            phone_number=phone_number,
            owner_id=int(owner_id),
            claude_cli=os.getenv("CLAUDE_CLI", "claude"),
            inference_timeout=int(os.getenv("INFERENCE_TIMEOUT", "150")),
            session_name=os.getenv("SESSION_NAME", "sophia"),
            exec_enabled=os.getenv("EXEC_ENABLED", "true").lower() == "true",
            exec_allowed_commands=[c.strip() for c in allowed.split(",") if c.strip()],
        )
