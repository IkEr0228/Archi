// Hide console window in release builds (Windows GUI app).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use archi_backend_lib::commands::{self, StartupCliCreate, StartupCliPath};
use archi_backend_lib::operations::OperationRegistry;
use std::sync::Mutex;
use tauri::Manager;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let action = archi_backend_lib::cli_handler::parse_cli_action(&args, &cwd);

    // Fast-path headless extraction when invoked from context menu or CLI:
    match action {
        archi_backend_lib::cli_handler::CliAction::ExtractHere(ref path) => {
            match archi_backend_lib::cli_handler::execute_cli_extraction(
                path,
                archi_backend_lib::cli_handler::CliExtractTarget::Here,
            ) {
                Ok(_) => return,
                Err(err) if err.code == "password_required" => {
                    // Password protected: fall through to GUI window for interactive password entry.
                }
                Err(err) => {
                    archi_backend_lib::cli_handler::show_native_error_box("Archi", &err.message);
                    return;
                }
            }
        }
        archi_backend_lib::cli_handler::CliAction::ExtractTo(ref path) => {
            match archi_backend_lib::cli_handler::execute_cli_extraction(
                path,
                archi_backend_lib::cli_handler::CliExtractTarget::ToSubfolder,
            ) {
                Ok(_) => return,
                Err(err) if err.code == "password_required" => {
                    // Password protected: fall through to GUI window for interactive password entry.
                }
                Err(err) => {
                    archi_backend_lib::cli_handler::show_native_error_box("Archi", &err.message);
                    return;
                }
            }
        }
        archi_backend_lib::cli_handler::CliAction::AddZip(ref path) => {
            if let Err(err) = archi_backend_lib::cli_handler::execute_cli_add_zip(path) {
                archi_backend_lib::cli_handler::show_native_error_box("Archi", &err.message);
            }
            return;
        }
        archi_backend_lib::cli_handler::CliAction::Add7z(ref path) => {
            if let Err(err) = archi_backend_lib::cli_handler::execute_cli_add_7z(path) {
                archi_backend_lib::cli_handler::show_native_error_box("Archi", &err.message);
            }
            return;
        }
        _ => {}
    }

    tauri::Builder::default()
        // Single-instance: handle secondary CLI invocations (open window, extract, or focus).
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let act =
                archi_backend_lib::cli_handler::parse_cli_action(&argv, std::path::Path::new(&cwd));
            match act {
                archi_backend_lib::cli_handler::CliAction::ExtractHere(ref path) => {
                    if let Err(err) = archi_backend_lib::cli_handler::execute_cli_extraction(
                        path,
                        archi_backend_lib::cli_handler::CliExtractTarget::Here,
                    ) {
                        if err.code == "password_required" {
                            let _ = archi_backend_lib::window_manager::create_new_window(
                                app,
                                Some(path.to_string_lossy().into_owned()),
                            );
                        } else {
                            archi_backend_lib::cli_handler::show_native_error_box(
                                "Archi",
                                &err.message,
                            );
                        }
                    }
                }
                archi_backend_lib::cli_handler::CliAction::ExtractTo(ref path) => {
                    if let Err(err) = archi_backend_lib::cli_handler::execute_cli_extraction(
                        path,
                        archi_backend_lib::cli_handler::CliExtractTarget::ToSubfolder,
                    ) {
                        if err.code == "password_required" {
                            let _ = archi_backend_lib::window_manager::create_new_window(
                                app,
                                Some(path.to_string_lossy().into_owned()),
                            );
                        } else {
                            archi_backend_lib::cli_handler::show_native_error_box(
                                "Archi",
                                &err.message,
                            );
                        }
                    }
                }
                archi_backend_lib::cli_handler::CliAction::AddZip(ref path) => {
                    if let Err(err) = archi_backend_lib::cli_handler::execute_cli_add_zip(path) {
                        archi_backend_lib::cli_handler::show_native_error_box(
                            "Archi",
                            &err.message,
                        );
                    }
                }
                archi_backend_lib::cli_handler::CliAction::Add7z(ref path) => {
                    if let Err(err) = archi_backend_lib::cli_handler::execute_cli_add_7z(path) {
                        archi_backend_lib::cli_handler::show_native_error_box(
                            "Archi",
                            &err.message,
                        );
                    }
                }
                archi_backend_lib::cli_handler::CliAction::Create(ref paths) => {
                    if let Some(first) = paths.first() {
                        let _ = archi_backend_lib::window_manager::create_new_window_with_target(
                            app,
                            archi_backend_lib::window_manager::WindowInitialTarget::Create(
                                first.to_string_lossy().into_owned(),
                            ),
                        );
                    }
                }
                archi_backend_lib::cli_handler::CliAction::Open(archive_path) => {
                    if let Err(error) = archi_backend_lib::window_manager::create_new_window(
                        app,
                        Some(archive_path.to_string_lossy().into_owned()),
                    ) {
                        eprintln!("Failed to spawn new window: {error}");
                    }
                }
                archi_backend_lib::cli_handler::CliAction::Blank => {
                    let windows = app.webview_windows();
                    if let Some(window) = windows
                        .values()
                        .find(|w| w.is_focused().unwrap_or(false))
                        .or_else(|| windows.values().next())
                    {
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    } else if let Err(error) =
                        archi_backend_lib::window_manager::create_new_window(app, None)
                    {
                        eprintln!("Failed to spawn blank window: {error}");
                    }
                }
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        .manage(OperationRegistry::default())
        .manage(StartupCliPath(Mutex::new(None)))
        .manage(StartupCliCreate(Mutex::new(None)))
        .setup(move |app| {
            let (startup_path, startup_create) = match action {
                archi_backend_lib::cli_handler::CliAction::Open(ref p) => {
                    (Some(p.to_string_lossy().into_owned()), None)
                }
                archi_backend_lib::cli_handler::CliAction::ExtractHere(ref p)
                | archi_backend_lib::cli_handler::CliAction::ExtractTo(ref p) => {
                    (Some(p.to_string_lossy().into_owned()), None)
                }
                archi_backend_lib::cli_handler::CliAction::Create(ref paths) => {
                    let list: Vec<String> = paths
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                    (None, Some(list))
                }
                _ => (None, None),
            };
            if let Ok(mut guard) = app.state::<StartupCliPath>().0.lock() {
                *guard = startup_path;
            }
            if let Ok(mut guard) = app.state::<StartupCliCreate>().0.lock() {
                *guard = startup_create;
            }

            archi_backend_lib::drag_out::cleanup_old_drag_temp_dirs();

            // Restore saved window state for main window if available:
            if let Some(window) = app.get_webview_window("main") {
                if let Some(saved) =
                    archi_backend_lib::window_manager::load_window_state(app.handle())
                {
                    let _ = window.set_position(tauri::Position::Physical(
                        tauri::PhysicalPosition::new(saved.x, saved.y),
                    ));
                    let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
                        saved.width,
                        saved.height,
                    )));
                }
                archi_backend_lib::window_manager::attach_window_state_saver(&window);
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_name,
            commands::get_startup_cli_path,
            commands::get_startup_cli_create,
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
            commands::move_archive_entries_command,
            commands::compact_archive_command,
            commands::select_archive_file,
            commands::select_directory,
            commands::select_save_archive,
            commands::select_multiple_files,
            commands::ensure_directory,
            commands::get_file_association_status_command,
            commands::register_file_associations_command,
            commands::unregister_file_associations_command,
            commands::start_drag_out,
            commands::prepare_drag_out,
            commands::cancel_drag_out,
            commands::create_new_window_command,
            commands::set_window_title_command,
            commands::get_context_menu_status_command,
            commands::register_context_menu_command,
            commands::unregister_context_menu_command,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
