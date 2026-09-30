fn main() {
    // Every command is listed, so that each gets its own permission: the
    // settings window is granted the launcher's commands (capability
    // "default"), a Kynoko window's page only the own_window_* ones and
    // system_fonts (capability "kynoko-window").
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
        "get_state",
        "set_default_browser",
        "set_default_profile",
        "set_app_browser",
        "set_app_profile",
        "set_associated",
        "set_extension",
        "set_shortcut_item",
        "app_icon",
        "set_shortcuts",
        "check_catalogue",
        "launch",
        "remove_everything",
        "open_default_apps",
        "open_download",
        "own_window_drag",
        "own_window_minimize",
        "own_window_toggle_maximize",
        "own_window_is_maximized",
        "own_window_close",
        "system_fonts",
    ])))
    .expect("failed to run the tauri build script")
}
