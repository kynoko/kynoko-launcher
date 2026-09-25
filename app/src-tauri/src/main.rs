// No console window behind the launcher in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    kynoko_launcher_lib::run()
}
