// Every command the UI may call is declared here. Tauri then refuses any
// command that is not also granted in capabilities/main.json, so a page that
// somehow ran foreign script still could not reach anything else.
const COMMANDS: &[&str] = &[
    "auth_get_session", "auth_sign_in", "auth_cancel", "auth_logout", "auth_get_usage",
    "check_version", "install_update", "auto_update", "apply_update",
    "fs_open_folder", "fs_read_tree", "fs_read_file", "fs_try_read_file", "fs_write_file",
    "fs_create_file", "fs_create_folder", "fs_rename", "fs_delete", "fs_search", "fs_replace",
    "fs_watch", "fs_unwatch", "get_recent_projects",
    "term_create", "term_write", "term_resize", "term_kill",
    "ai_send", "ai_cancel", "ai_get_models", "ai_refresh_models", "ai_run_tool", "ai_generate_media", "ai_image_suppliers", "ai_complete",
    "providers_list", "providers_save", "providers_delete", "providers_fetch_models",
    "ext_search", "ext_details", "ext_icon_theme", "ext_install", "ext_uninstall", "ext_list", "ext_contributions",
    "chat_list", "chat_load", "chat_save", "chat_delete",
    "settings_get", "settings_set",
    "open_external", "pick_media",
    "win_new", "win_open_file",
    "cli_status", "cli_install", "presence_set",
    "bots_list", "bots_save", "bots_delete", "bots_duplicate", "bots_threads", "bots_thread", "bots_set_memory", "bots_send", "bots_cancel", "bots_answer", "rooms_save", "rooms_delete", "rooms_get", "rooms_send", "bots_computer",
    "mobile_status", "mobile_set_password", "mobile_configure", "mobile_set_tunnel_token", "mobile_pair_qr", "mobile_revoke",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
