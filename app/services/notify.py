"""Notifications (Discord, ntfy, generic webhook) and media-server refresh."""
import hashlib
import secrets

from .. import config, db
from .http import client


async def send(title: str, message: str, image: str | None = None) -> None:
    c = client()
    if config.get("discord_webhook"):
        embed = {"title": title, "description": message, "color": 0x8B5CF6}
        if image:
            embed["thumbnail"] = {"url": image}
        try:
            await c.post(config.get("discord_webhook"), json={"username": config.get("app_name"), "embeds": [embed]})
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Discord notification failed: {e}")
    if config.get("ntfy_url"):
        headers = {"Title": title.encode("utf-8").decode("latin-1", "ignore"), "Tags": "musical_note"}
        if config.get("ntfy_token"):
            headers["Authorization"] = f"Bearer {config.get('ntfy_token')}"
        try:
            await c.post(config.get("ntfy_url"), content=message.encode(), headers=headers)
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"ntfy notification failed: {e}")
    if config.get("webhook_url"):
        try:
            await c.post(config.get("webhook_url"), json={"title": title, "message": message, "image": image})
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Webhook notification failed: {e}")


async def refresh_media_servers() -> list[str]:
    c = client()
    done = []
    if config.get("navidrome_url") and config.get("navidrome_user"):
        salt = secrets.token_hex(6)
        token = hashlib.md5((config.get("navidrome_password") + salt).encode()).hexdigest()
        try:
            await c.get(config.get("navidrome_url").rstrip("/") + "/rest/startScan", params={
                "u": config.get("navidrome_user"), "t": token, "s": salt, "v": "1.16.1", "c": "encore", "f": "json"})
            done.append("Navidrome")
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Navidrome scan failed: {e}")
    if config.get("jellyfin_url") and config.get("jellyfin_api_key"):
        try:
            await c.post(config.get("jellyfin_url").rstrip("/") + "/Library/Refresh", headers={"X-Emby-Token": config.get("jellyfin_api_key")})
            done.append("Jellyfin")
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Jellyfin refresh failed: {e}")
    if config.get("plex_url") and config.get("plex_token"):
        try:
            await c.get(config.get("plex_url").rstrip("/") + "/library/sections/all/refresh", params={"X-Plex-Token": config.get("plex_token")})
            done.append("Plex")
        except Exception as e:  # noqa: BLE001
            db.log("warning", f"Plex refresh failed: {e}")
    return done
