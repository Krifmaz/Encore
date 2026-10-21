-- Schema 28 (0.32): plugins. One row per plugin Encore has installed or adopted: the catalog's (first-party,
-- Encore's own code) and the uploaded or URL-installed ones (third-party). A catalog plugin without a row is
-- "available, not installed". encore-plugins keeps the rows (crates/plugins/src/registry.rs); the manifest
-- format is docs/contracts/plugin-manifest.md.
--
-- id: the manifest's [plugin] id (the folder <config>/plugins/<id>); version: the version installed (a process
-- plugin's own version, a catalog plugin's program version, NULL when it installs no program); enabled: 1/0;
-- installed_at, updated_at: epoch seconds; origin: 'catalog' | 'upload' | 'url'; trust: 'first-party' |
-- 'third-party'; manifest: the plugin.toml text of the installed version; sha256: of the zip or program
-- download it came from; auto_update: the plugin's auto-update switch (1/0); last_check: when its updates
-- were last looked for; last_error: what the last install, update or check said when it failed (NULL: fine).
-- A plugin's own settings are rows of the settings table named plugin.<id>.<key>.
CREATE TABLE IF NOT EXISTS plugins (
    id TEXT PRIMARY KEY,
    version TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    installed_at REAL,
    updated_at REAL,
    origin TEXT NOT NULL DEFAULT 'catalog',
    trust TEXT NOT NULL DEFAULT 'first-party',
    manifest TEXT,
    sha256 TEXT,
    auto_update INTEGER NOT NULL DEFAULT 0,
    last_check REAL,
    last_error TEXT
);

-- Which start this is for the plugins (encore-plugins, crates/plugins/src/migrate.rs), written once: "adopt" on a
-- database that was in use before plugins (it has accounts or settings), so the first start adopts and installs
-- what was in use and downloads keep working with no new setup; "fresh" on a new database, where every plugin
-- is "available, not installed". The registry moves it on to "install" and "done". Kept out of the settings
-- table: a fresh database must start with no settings stored (Config tells a fresh install by that).
CREATE TABLE IF NOT EXISTS plugin_state (key TEXT PRIMARY KEY, value TEXT);
INSERT OR IGNORE INTO plugin_state (key, value)
    SELECT 'migration', CASE WHEN EXISTS (SELECT 1 FROM users) OR EXISTS (SELECT 1 FROM settings) THEN 'adopt' ELSE 'fresh' END;
