//! Blaze IDE desktop shell (Tauri 2). All logic lives in blaze-core; this
//! crate wires it to windows, menus, the OS keychain and the webview.

mod analytics;
mod blbots;
mod cli_install;
mod commands;
mod media;
mod menu;
mod mobile;
mod presence;
mod secrets;
mod windows;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, RwLock};

use blaze_core::models::ModelEntry;
use blaze_core::paths::Sandbox;
use blaze_core::pty::PtyManager;
use blaze_core::store::Store;
use blaze_core::watch::WatchManager;
use tauri::Manager;
use tokio_util::sync::CancellationToken;

pub struct AppState {
    pub sandbox: Sandbox,
    pub store: Store,
    pub pty: PtyManager,
    pub watch: WatchManager,
    pub http: blaze_core::reqwest::Client,
    /// For user-added providers (allows http to localhost).
    pub http_custom: blaze_core::reqwest::Client,
    pub api_base: String,
    /// Host of the `/paid/v1` API (models, chat, images, usage).
    pub v1_base: String,
    /// Installed VS Code extensions (themes, snippets, language configs).
    pub ext_dir: std::path::PathBuf,
    pub key: RwLock<Option<String>>,
    pub models: RwLock<Vec<ModelEntry>>,
    pub ai_requests: Mutex<HashMap<String, CancellationToken>>,
    pub signin: Mutex<Option<CancellationToken>>,
    /// The update found by the last check, kept for the install step.
    pub update: Mutex<Option<tauri_plugin_updater::Update>>,
    /// An update downloaded (and signature-checked) in the background, waiting
    /// to be installed: on Restart, or when the app quits (Windows).
    pub update_ready: Mutex<Option<(tauri_plugin_updater::Update, Vec<u8>)>>,
    /// Last dev server shown through `blazeweb:` (for root-relative URLs).
    pub web_last: Mutex<Option<String>>,
    /// The app's copies of picked backgrounds / ambient audio (see media_store).
    pub media_dir: std::path::PathBuf,
}

impl AppState {
    pub fn api_key(&self) -> Option<String> {
        self.key.read().expect("key lock").clone()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // A second launch focuses the existing app instead of starting another
        // copy that would fight over the sign-in port and the state file.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol("blazemedia", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                responder.respond(media::handle(&state.sandbox, &request));
            });
        })
        .register_asynchronous_uri_scheme_protocol("blazeweb", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || {
                let state = app.state::<AppState>();
                responder.respond(media::proxy(&state.http_custom, &state.web_last, &request));
            });
        })
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let store = Store::open(&dir);
            let sandbox = Sandbox::new();
            // Projects the user opened before stay trusted across restarts.
            for p in store.recent_projects() {
                let _ = sandbox.grant_root(Path::new(&p));
            }
            // Picked backgrounds/audio live in the app's media folder, which is
            // readable on every launch (they used to vanish after a restart).
            let media_dir = blaze_core::media_store::media_dir(&dir);
            let _ = sandbox.grant_media_dir(&media_dir);
            let (patch, legacy) = blaze_core::media_store::migrate(&media_dir, &store.settings());
            if !patch.is_empty() {
                let _ = store.patch_settings(patch);
            }
            for p in legacy {
                let _ = sandbox.grant_picked_file(&p);
            }
            blaze_core::media_store::prune(&media_dir, &blaze_core::media_store::referenced(&store.settings()));
            app.manage(AppState {
                sandbox,
                store,
                pty: PtyManager::new(),
                watch: WatchManager::new(),
                http: blaze_core::http_client(),
                http_custom: blaze_core::custom_http_client(),
                api_base: blaze_core::api_base(),
                v1_base: blaze_core::v1_base(),
                ext_dir: dir.join("extensions"),
                key: RwLock::new(secrets::load_key()),
                models: RwLock::new(Vec::new()),
                ai_requests: Mutex::new(HashMap::new()),
                signin: Mutex::new(None),
                update: Mutex::new(None),
                update_ready: Mutex::new(None),
                web_last: Mutex::new(None),
                media_dir,
            });
            let handle = app.handle().clone();
            app.set_menu(menu::build(&handle)?)?;
            app.on_menu_event(|app, ev| menu::on_event(app, ev.id().as_ref()));
            windows::open(&handle, None)?;
            commands::on_launch(&handle);
            // Continue on mobile: start the phone server if the user turned it on.
            let h2 = handle.clone();
            tauri::async_runtime::spawn(async move { mobile::apply(&h2).await });
            // BlBots routines and check-backs run while the app is open.
            blbots::start_scheduler(&handle);
            presence::sync(&handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::auth_get_session,
            commands::auth_sign_in,
            commands::auth_cancel,
            commands::auth_logout,
            commands::auth_get_usage,
            commands::check_version,
            commands::install_update,
            commands::auto_update,
            commands::apply_update,
            commands::fs_open_folder,
            commands::fs_read_tree,
            commands::fs_read_file,
            commands::fs_try_read_file,
            commands::fs_write_file,
            commands::fs_create_file,
            commands::fs_create_folder,
            commands::fs_rename,
            commands::fs_delete,
            commands::fs_search,
            commands::fs_replace,
            commands::fs_watch,
            commands::fs_unwatch,
            commands::get_recent_projects,
            commands::term_create,
            commands::term_write,
            commands::term_resize,
            commands::term_kill,
            commands::ai_send,
            commands::ai_cancel,
            commands::ai_get_models,
            commands::ai_refresh_models,
            commands::ai_run_tool,
            commands::ai_generate_media,
            commands::ai_image_suppliers,
            commands::ai_complete,
            commands::providers_list,
            commands::providers_save,
            commands::providers_delete,
            commands::providers_fetch_models,
            commands::ext_search,
            commands::ext_install,
            commands::ext_uninstall,
            commands::ext_list,
            commands::ext_contributions,
            commands::chat_list,
            commands::chat_load,
            commands::chat_save,
            commands::chat_delete,
            commands::settings_get,
            commands::settings_set,
            commands::open_external,
            commands::pick_media,
            commands::cli_status,
            presence::presence_set,
            blbots::bots_list,
            blbots::bots_save,
            blbots::bots_delete,
            blbots::bots_duplicate,
            blbots::bots_threads,
            blbots::bots_thread,
            blbots::bots_set_memory,
            blbots::bots_send,
            blbots::bots_cancel,
            blbots::bots_answer,
            blbots::rooms_save,
            blbots::rooms_delete,
            blbots::rooms_get,
            blbots::rooms_send,
            blbots::bots_computer,
            commands::cli_install,
            commands::mobile_status,
            commands::mobile_set_password,
            commands::mobile_configure,
            commands::mobile_set_tunnel_token,
            commands::mobile_pair_qr,
            commands::mobile_revoke,
            commands::win_new,
            commands::win_open_file,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Blaze IDE")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                state.pty.kill_all();
                state.watch.unwatch_all();
                // A background-downloaded update that was not installed yet
                // (Windows: the installer must close the app) goes in now, so
                // the next start is the new version.
                let ready = state.update_ready.lock().ok().and_then(|mut g| g.take());
                if let Some((update, bytes)) = ready {
                    let _ = update.install(&bytes);
                }
            }
        });
}
