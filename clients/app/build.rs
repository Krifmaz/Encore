// The app's own commands get permissions (allow-<command>), so capabilities/ can hand a few of them to the
// Encore server's pages (remote origin) and all of them to the app's own pages.
const COMMANDS: &[&str] = &["now_playing", "page_alive", "servers", "connect", "remove_server", "change_server", "retry_server",
    "open_offline", "offline_list", "offline_ids", "remove_offline", "save_offline", "engine_login", "engine_logout", "engine_status"];

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)))
        .expect("tauri build");
}
