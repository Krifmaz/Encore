const COMMANDS: &[&str] = &["now_playing", "take_share", "cookie", "set_server"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).android_path("android").build();
}
