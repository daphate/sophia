import asyncio

from bot.main import run


def main():
    try:
        asyncio.run(run())
    except KeyboardInterrupt:
        print("\nSophia stopped.")


if __name__ == "__main__":
    main()
