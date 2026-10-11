// Desktop entry point (Windows, macOS, Linux). Android starts through the mobile entry point in lib.rs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    encore_app_lib::run()
}
