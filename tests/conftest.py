import os
import subprocess
import sys

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


@pytest.fixture()
def env(tmp_path, monkeypatch):
    monkeypatch.setenv("ENCORE_CONFIG_DIR", str(tmp_path / "config"))
    monkeypatch.setenv("ENCORE_LIBRARY_PATH", str(tmp_path / "music"))
    monkeypatch.setenv("ENCORE_STAGING_PATH", str(tmp_path / "staging"))
    monkeypatch.setenv("ENCORE_ADMIN_PASSWORD", "adminpass123")
    monkeypatch.setenv("ENCORE_SCAN_INTERVAL", "0")
    import importlib

    from app import auth, db
    auth._attempts.clear()
    db.DATA_DIR = str(tmp_path / "config")
    db.DB_PATH = os.path.join(db.DATA_DIR, "encore.db")
    from app import config
    importlib.reload(config)
    config._cache.clear()
    return tmp_path


@pytest.fixture()
def client(env):
    from fastapi.testclient import TestClient

    from app.main import app
    with TestClient(app) as c:
        yield c


def make_audio(path, title="Song", artist="Artist", album="Album", track=1, seconds=2):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    subprocess.run(
        ["ffmpeg", "-loglevel", "error", "-y", "-f", "lavfi", "-i", f"sine=frequency=440:duration={seconds}",
         "-metadata", f"title={title}", "-metadata", f"artist={artist}", "-metadata", f"album={album}",
         "-metadata", f"track={track}", path],
        check=True,
    )
    return path
