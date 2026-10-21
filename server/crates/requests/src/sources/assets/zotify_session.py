"""Runs inside the plugin's zotify virtualenv (it needs librespot). Prints one JSON object.

    zotify_session.py login <access_token_file> <credentials.json>   a Spotify access token -> zotify's credentials.json
    zotify_session.py check <credentials.json>                        signs in with the saved credentials

Both answer {"ok": true, "username": ..., "product": "premium" | "free" | ...} or {"ok": false, "error": ...}.
"""
import json
import os
import sys
import time


def account(session) -> dict:
    product = None
    for _ in range(30):  # the product info packet arrives shortly after the welcome
        product = session.get_user_attribute("type")
        if product:
            break
        time.sleep(0.1)
    return {"ok": True, "username": session.username(), "product": product or "unknown"}


def save(session, path: str) -> None:
    tmp = path + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump(session.credentials(), f)
    os.replace(tmp, path)


def main() -> int:
    from librespot.core import Session
    from librespot.proto import Authentication_pb2 as Authentication

    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    builder = Session.Builder()
    builder.conf.store_credentials = False
    if cmd == "login":
        with open(sys.argv[2]) as f:
            token = f.read().strip()
        builder.login_credentials = Authentication.LoginCredentials(
            typ=Authentication.AuthenticationType.AUTHENTICATION_SPOTIFY_TOKEN, auth_data=token.encode())
        creds_path = sys.argv[3]
    elif cmd == "check":
        creds_path = sys.argv[2]
        with open(creds_path) as f:
            obj = json.load(f)
        import base64
        builder.login_credentials = Authentication.LoginCredentials(
            typ=Authentication.AuthenticationType.Value(obj["type"]), username=obj["username"],
            auth_data=base64.b64decode(obj["credentials"]))
    else:
        print(json.dumps({"ok": False, "error": f"unknown command {cmd!r}"}))
        return 2
    session = builder.create()
    try:
        out = account(session)
        save(session, creds_path)
    finally:
        try:
            session.close()
        except Exception:  # noqa: BLE001 - closing is best effort
            pass
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:  # noqa: BLE001 - the plugin reads the JSON line
        print(json.dumps({"ok": False, "error": f"{type(e).__name__}: {e}"}))
        sys.exit(1)
