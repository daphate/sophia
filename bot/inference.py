import asyncio
import json
import logging
import subprocess

from bot.config import Config
from bot.memory import build_system_prompt, load_recent_dialog

logger = logging.getLogger(__name__)


def _run_claude_sync(cmd: list[str], message: str, timeout: int) -> tuple[int, str, str]:
    """Run Claude CLI synchronously (called via asyncio.to_thread)."""
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        stdout, stderr = proc.communicate(input=message, timeout=timeout)
        return proc.returncode, stdout, stderr
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)
        raise


def _parse_claude_output(raw: str) -> tuple[str, dict | None]:
    """Parse Claude CLI JSON array output. Returns (text, cost_info).

    With tools enabled, Claude may produce multiple assistant turns
    (thinking, tool calls, then final answer). We collect only the
    text blocks from the *last* assistant message as the user-facing
    response, plus the result text if present.
    """
    data = json.loads(raw)

    if not isinstance(data, list):
        data = [data]

    # Collect text from all assistant messages; we'll use the last one
    assistant_texts: list[list[str]] = []
    result_text = ""
    total_input = 0
    total_output = 0
    cost_usd = None

    for item in data:
        item_type = item.get("type")

        if item_type == "assistant":
            msg = item.get("message", {})
            texts = []
            for block in msg.get("content", []):
                if block.get("type") == "text":
                    texts.append(block["text"])
            if texts:
                assistant_texts.append(texts)
            usage = msg.get("usage", {})
            total_input += usage.get("input_tokens", 0)
            total_output += usage.get("output_tokens", 0)

        elif item_type == "result":
            # result.result contains the final text summary
            if item.get("result"):
                result_text = item["result"]
            if item.get("cost_usd") is not None:
                cost_usd = item["cost_usd"]
            # result also has total usage
            total_input = item.get("total_input_tokens", total_input)
            total_output = item.get("total_output_tokens", total_output)

    # Prefer result text (final answer after tool use), fall back to last assistant message
    if result_text:
        final = result_text
    elif assistant_texts:
        final = "\n".join(assistant_texts[-1])
    else:
        final = raw

    cost = {
        "input_tokens": total_input,
        "output_tokens": total_output,
        "cost_usd": cost_usd,
    }

    return final, cost


async def ask_claude(
    user_id: int, message: str, config: Config,
    file_paths: list[str] | None = None,
) -> tuple[str, dict | None]:
    """Call Claude CLI and return (response_text, cost_info).

    If file_paths is provided, each file is referenced in the prompt so that
    Claude CLI can read them via its built-in Read tool.
    """
    recent = load_recent_dialog(user_id)
    system_prompt = build_system_prompt(recent)

    # Build prompt with file references
    prompt_parts = []
    if file_paths:
        for fp in file_paths:
            prompt_parts.append(f"[Attached file: {fp}]")
        prompt_parts.append("")  # blank line separator

    prompt_parts.append(message)
    full_message = "\n".join(prompt_parts)

    cmd = [
        config.claude_cli,
        "-p",
        "--output-format", "json",
        "--verbose",
        "--dangerously-skip-permissions",
        "--system-prompt", system_prompt,
    ]

    try:
        returncode, stdout, stderr = await asyncio.to_thread(
            _run_claude_sync, cmd, full_message, config.inference_timeout
        )
    except subprocess.TimeoutExpired:
        logger.error("Claude CLI timed out after %ds", config.inference_timeout)
        return "Sorry, the request timed out. Please try again.", None
    except FileNotFoundError:
        logger.error("Claude CLI not found at: %s", config.claude_cli)
        return "Error: Claude CLI not found. Check CLAUDE_CLI config.", None

    if returncode != 0:
        err = stderr.strip() if stderr else "(no stderr)"
        logger.error("Claude CLI error (rc=%d): %s", returncode, err[:500])
        return f"Error from Claude: {err[:500]}", None

    raw = stdout.strip() if stdout else ""

    try:
        return _parse_claude_output(raw)
    except (json.JSONDecodeError, KeyError, TypeError) as e:
        logger.warning("Failed to parse Claude output (%s), returning raw", e)
        return raw, None
