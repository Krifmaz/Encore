"""Outgoing email over the admin's SMTP server (password resets, test mail)."""
import asyncio
import smtplib
import ssl
from email.message import EmailMessage
from email.utils import parseaddr

from .. import config


def enabled() -> bool:
    return bool(config.get("smtp_host"))


def _send(to: str, subject: str, text: str) -> None:
    host, port = config.get("smtp_host"), int(config.get("smtp_port") or 587)
    mode = config.get("smtp_security") or "starttls"
    sender = config.get("smtp_from") or config.get("smtp_user")
    msg = EmailMessage()
    msg["From"], msg["To"], msg["Subject"] = sender, to, subject
    msg.set_content(text)
    ctx = ssl.create_default_context()
    if mode == "ssl":
        s = smtplib.SMTP_SSL(host, port, context=ctx, timeout=20)
    else:
        s = smtplib.SMTP(host, port, timeout=20)
        if mode == "starttls":
            s.starttls(context=ctx)
    with s:
        if config.get("smtp_user"):
            s.login(config.get("smtp_user"), config.get("smtp_password"))
        s.send_message(msg, from_addr=parseaddr(sender)[1] or sender)


async def send(to: str, subject: str, text: str) -> None:
    if not enabled():
        raise RuntimeError("Email isn't set up – add an SMTP server under Settings → Email & notifications")
    await asyncio.to_thread(_send, to, subject, text)
