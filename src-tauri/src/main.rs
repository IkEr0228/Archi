use archi_backend_lib::cli_open::resolve_cli_archive_path;
use archi_backend_lib::commands::{self, StartupCliPath};
use archi_backend_lib::operations::OperationRegistry;
use std::sync::Mutex;
use tauri::{Emitter, Manager};

#[derive(Clone, serde::Serialize)]
struct CliOpenPayload {
    path: Option<String>,
}

fn main() {
    tauri::Builder::default()
        // Single-instance must register first so secondary launches exit cleanly.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let path = resolve_cli_archive_path(&argv, std::path::Path::new(&cwd))
                .map(|p| p.to_string_lossy().into_owned());
            if let Err(error) = app.emit("cli-open", CliOpenPayload { path }) {
                eprintln!("Failed to emit cli-open: {error}");
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(OperationRegistry::default())
        .manage(StartupCliPath(Mutex::new(None)))
        .setup(|app| {
            let args: Vec<String> = std::env::args().collect();
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let path =
                resolve_cli_archive_path(&args, &cwd).map(|p| p.to_string_lossy().into_owned());
            if let Ok(mut guard) = app.state::<StartupCliPath>().0.lock() {
                *guard = path;
            }

            #[cfg(any(target_os = "windows", target_os = "macos"))]
            {
                let window = app.get_webview_window("main").unwrap();

                #[cfg(target_os = "windows")]
                {
                    let _ = window_vibrancy::apply_acrylic(&window, Some((245, 235, 235, 100)));
                }

                #[cfg(target_os = "macos")]
                {
                    let _ = window_vibrancy::apply_vibrancy(
                        &window,
                        window_vibrancy::NSVisualEffectMaterial::UnderWindowBackground,
                        None,
                        None,
                    );
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_name,
            commands::get_startup_cli_path,
            commands::open_archive_metadata,
            commands::test_archive_command,
            commands::extract_archive_command,
            commands::resolve_extract_conflict,
            commands::cancel_operation,
            commands::create_archive_command,
            commands::delete_archive_entries_command,
            commands::rename_archive_entry_command,
            commands::create_archive_folder_command,
            commands::add_to_archive_command,
            commands::replace_archive_file_command,
            commands::select_archive_file,
            commands::select_directory,
            commands::select_save_archive,
            commands::select_multiple_files,
            commands::ensure_directory,
            commands::get_file_association_status_command,
            commands::register_file_associations_command,
            commands::unregister_file_associations_command,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
