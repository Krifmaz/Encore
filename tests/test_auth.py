H = {"X-Requested-With": "encore"}


def login(c, u="admin", p="adminpass123"):
    r = c.post("/api/auth/login", json={"username": u, "password": p})
    assert r.status_code == 200, r.text
    return r


def test_requires_login(client):
    assert client.get("/api/library/stats").status_code == 401
    assert client.get("/api/auth/info").json()["signup_mode"] == "invite"


def test_login_and_csrf(client):
    assert client.post("/api/auth/login", json={"username": "admin", "password": "wrong"}).status_code == 401
    login(client)
    assert client.get("/api/auth/me").json()["role"] == "admin"
    # mutating call without CSRF header is rejected
    assert client.post("/api/admin/invites", json={}).status_code == 403
    assert client.post("/api/admin/invites", json={}, headers=H).status_code == 200


def test_invite_signup_flow(client):
    login(client)
    inv = client.post("/api/admin/invites", json={"max_uses": 1}, headers=H).json()
    client.post("/api/auth/logout", headers=H)
    client.cookies.clear()
    # no invite → refused in invite mode
    r = client.post("/api/auth/signup", json={"username": "bob", "password": "bobpassword"})
    assert r.status_code == 403
    r = client.post("/api/auth/signup", json={"username": "bob", "password": "bobpassword", "invite": inv["code"]})
    assert r.status_code == 200, r.text
    assert r.json()["role"] == "user"
    # regular users can't reach admin endpoints
    assert client.get("/api/admin/users").status_code == 403
    client.cookies.clear()
    # invite is used up
    r = client.post("/api/auth/signup", json={"username": "eve", "password": "evepassword", "invite": inv["code"]})
    assert r.status_code == 400


def test_closed_and_open_modes(client):
    login(client)
    client.put("/api/admin/settings", json={"signup_mode": "open"}, headers=H)
    client.cookies.clear()
    assert client.post("/api/auth/signup", json={"username": "ann", "password": "annpassword"}).status_code == 200
    client.cookies.clear()
    login(client)
    client.put("/api/admin/settings", json={"signup_mode": "closed"}, headers=H)
    client.cookies.clear()
    assert client.post("/api/auth/signup", json={"username": "zed", "password": "zedpassword"}).status_code == 403


def test_disable_user_kills_session(client):
    login(client)
    uid = client.post("/api/admin/users", json={"username": "carl", "password": "carlpassword"}, headers=H).json()["id"]
    from fastapi.testclient import TestClient

    from app.main import app
    other = TestClient(app)
    login(other, "carl", "carlpassword")
    assert other.get("/api/auth/me").status_code == 200
    client.patch(f"/api/admin/users/{uid}", json={"disabled": True}, headers=H)
    assert other.get("/api/auth/me").status_code == 401


def test_settings_mask_secrets(client):
    login(client)
    client.put("/api/admin/settings", json={"lidarr_api_key": "secret123"}, headers=H)
    s = client.get("/api/admin/settings").json()
    assert s["lidarr_api_key"] == "••••••••"
    # saving the mask back must not overwrite the secret
    client.put("/api/admin/settings", json={"lidarr_api_key": "••••••••"}, headers=H)
    from app import config
    assert config.get("lidarr_api_key") == "secret123"


def test_api_key_header(client):
    login(client)
    key = client.get("/api/system/api-key").json()["api_key"]
    client.cookies.clear()
    r = client.get("/api/auth/me", headers={"X-Api-Key": key})
    assert r.status_code == 200 and r.json()["role"] == "admin"


def test_request_quota_and_approval(client):
    login(client)
    client.post("/api/admin/users", json={"username": "dina", "password": "dinapassword"}, headers=H)
    uid = [u for u in client.get("/api/admin/users").json() if u["username"] == "dina"][0]["id"]
    client.patch(f"/api/admin/users/{uid}", json={"request_limit": 1}, headers=H)
    client.cookies.clear()
    login(client, "dina", "dinapassword")
    r = client.post("/api/requests", json={"kind": "url", "url": "https://example.com/a"}, headers=H)
    assert r.status_code == 200 and r.json()["status"] == "pending"
    r = client.post("/api/requests", json={"kind": "url", "url": "https://example.com/b"}, headers=H)
    assert r.status_code == 429


def test_library_endpoints(client, env):
    from conftest import make_audio
    login(client)
    make_audio(str(env / "music" / "A" / "B" / "01.mp3"), title="Song", artist="A", album="B", track=1)
    from app.services import library
    import asyncio
    asyncio.run(library.scan())
    albums = client.get("/api/library/albums").json()
    assert albums["total"] == 1
    aid = albums["items"][0]["id"]
    detail = client.get(f"/api/library/albums/{aid}").json()
    tid = detail["tracks"][0]["id"]
    assert client.get(f"/api/library/tracks/{tid}/stream").status_code == 200
    z = client.get(f"/api/library/albums/{aid}/download")
    assert z.status_code == 200 and z.content[:2] == b"PK"
    r = client.post("/api/library/tags", json={"track_ids": [tid], "values": {"title": "Renamed"}}, headers=H)
    assert r.status_code == 200
    assert client.get(f"/api/library/albums/{r.json()['album_id']}").json()["tracks"][0]["title"] == "Renamed"
