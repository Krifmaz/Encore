// The app's own commands get permissions (allow-<command>), so capabilities/ can hand a few of them to the
// Encore server's pages (remote origin) and all of them to the app's own pages.
const COMMANDS: &[&str] = &["now_playing", "page_alive", "servers", "connect", "remove_server", "change_server", "retry_server",
    "open_offline", "offline_list", "offline_ids", "remove_offline", "save_offline", "engine_login", "engine_logout", "engine_status",
    // desktop only (src/extras.rs, src/engine.rs): This computer settings, mini player, updates, offline songs through Cadenza
    "desktop_settings", "set_desktop_settings", "mini_player", "mini_state", "player_control", "check_update", "install_update",
    "offline_play_songs", "offline_engine", "offline_engine_status"];

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)))
        .expect("tauri build");
}
