use crate::archive::open_archive;
use crate::extraction::{extract_any, ConflictResolver};
use crate::file_assoc::{
    get_file_association_status, register_file_associations, unregister_file_associations,
    FileAssociationStatus,
};

use crate::archive_edit::{
    add_paths, compact_archive, create_folder, delete_entries, move_entries, rename_entry,
    replace_file,
};
use crate::models::{
    ArchiveInfo, CommandError, ConflictDecision, CreateFormat, CreateOptions, EditOptions,
    EditSummary, ExtractConflictEvent, OperationSummary, TestArchiveSummary,
};
use crate::operations::OperationRegistry;
use crate::security::is_link_or_reparse_point;
use crate::sevenz_format::create_sevenz_archive;
use crate::tar_create::create_tar_archive;
use crate::testing::test_archive;
use crate::zipper::create_zip_archive;
use sevenz_rust2::Password;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{command, AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

/// CLI archive path resolved at process startup (first instance).
pub struct StartupCliPath(pub Mutex<Option<String>>);

/// Production conflict resolver: apply-to-all policy, then UI via extract-conflict + wait.
struct RegistryConflictResolver {
    registry: OperationRegistry,
    app: AppHandle,
}

impl ConflictResolver for RegistryConflictResolver {
    fn resolve_file_exists(
        &self,
        operation_id: &str,
        entry_path: &str,
        dest_path: &Path,
    ) -> Result<ConflictDecision, CommandError> {
        if let Some(policy) = self.registry.peek_apply_policy(operation_id) {
            return Ok(policy);
        }

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let conflict_id = format!("{operation_id}-{nanos}");

        // Register waiter before emit so a fast UI response cannot race past the channel.
        self.registry
            .install_conflict_waiter(operation_id, &conflict_id)
            .map_err(|message| CommandError::new("operation_failed", message))?;

        let event = ExtractConflictEvent {
            operation_id: operation_id.to_string(),
            conflict_id: conflict_id.clone(),
            entry_path: entry_path.to_string(),
            dest_path: dest_path.to_string_lossy().into_owned(),
        };
        if let Err(error) = self.app.emit("extract-conflict", event) {
            eprintln!("Failed to emit extract-conflict: {error}");
        }

        self.registry
            .recv_conflict_decision(operation_id, &conflict_id)
            .map_err(|message| CommandError::new("operation_failed", message))
    }
}

#[command]
pub fn get_app_name() -> String {
    env!("CARGO_PKG_NAME").to_string()
}

/// Archive path from the first process argv, if any.
#[command]
pub fn get_startup_cli_path(state: State<'_, StartupCliPath>) -> Option<String> {
    state.0.lock().ok().and_then(|guard| guard.clone())
}

#[command]
pub async fn open_archive_metadata(
    path: String,
    password: Option<String>,
) -> Result<ArchiveInfo, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        open_archive(std::path::Path::new(&path), password)
    })
    .await
    .map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

#[command]
pub async fn test_archive_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    zip_path: String,
    password: Option<String>,
) -> Result<TestArchiveSummary, CommandError> {
    let state = registry
        .start(&operation_id)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        test_archive(
            std::path::Path::new(&zip_path),
            &worker_operation_id,
            &cancelled,
            password,
            move |progress| {
                if let Err(error) = progress_app.emit("test-progress", progress) {
                    eprintln!("Failed to emit test-progress: {error}");
                }
            },
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

#[command]
pub async fn extract_archive_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    zip_path: String,
    dest_dir: String,
    selected_paths: Option<Vec<String>>,
    password: Option<String>,
) -> Result<crate::models::OperationSummary, CommandError> {
    let state = registry
        .start(&operation_id)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_registry = registry.inner().clone();
    let worker_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let selected_ref = selected_paths.as_deref();
        let resolver = RegistryConflictResolver {
            registry: worker_registry,
            app: worker_app,
        };
        extract_any(
            std::path::Path::new(&zip_path),
            std::path::Path::new(&dest_dir),
            &worker_operation_id,
            &cancelled,
            selected_ref,
            password,
            &resolver,
            move |progress| {
                if let Err(error) = progress_app.emit("extract-progress", progress) {
                    eprintln!("Failed to emit extraction progress: {error}");
                }
            },
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

#[command]
pub fn resolve_extract_conflict(
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    conflict_id: String,
    decision: ConflictDecision,
    apply_to_all: bool,
) -> Result<(), CommandError> {
    registry
        .resolve_conflict(&operation_id, &conflict_id, decision, apply_to_all)
        .map_err(|message| CommandError::new("invalid_operation", message))
}

#[command]
pub fn cancel_operation(registry: State<'_, OperationRegistry>, operation_id: String) -> bool {
    registry.cancel(&operation_id)
}

#[command]
pub async fn create_archive_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    source_paths: Vec<String>,
    output_zip_path: String,
    options: CreateOptions,
) -> Result<OperationSummary, CommandError> {
    let state = registry
        .start(&operation_id)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let password = match &options.password {
        Some(pw) => Password::new(pw),
        None => Password::empty(),
    };
    let result = tauri::async_runtime::spawn_blocking(move || {
        let path = std::path::Path::new(&output_zip_path);
        let emit = move |progress| {
            if let Err(error) = progress_app.emit("create-progress", progress) {
                eprintln!("Failed to emit creation progress: {error}");
            }
        };
        match options.format {
            CreateFormat::Zip => create_zip_archive(
                &source_paths,
                path,
                &worker_operation_id,
                &cancelled,
                &options,
                emit,
            ),
            CreateFormat::Tar
            | CreateFormat::TarGz
            | CreateFormat::TarBz2
            | CreateFormat::TarXz => {
                if options.password.is_some() {
                    // TAR-family formats have no native encryption: write a real
                    // AES-256 7z and fix the output extension to match the bytes.
                    let adjusted_path = path.with_extension("7z");
                    create_sevenz_archive(
                        &source_paths,
                        &adjusted_path,
                        &worker_operation_id,
                        &cancelled,
                        options
                            .password
                            .as_ref()
                            .map(|s| Password::new(s.as_str()))
                            .unwrap_or_else(Password::empty),
                        &options,
                        emit,
                    )
                } else {
                    create_tar_archive(
                        &source_paths,
                        path,
                        &worker_operation_id,
                        &cancelled,
                        &options,
                        emit,
                    )
                }
            }
            CreateFormat::SevenZ => create_sevenz_archive(
                &source_paths,
                path,
                &worker_operation_id,
                &cancelled,
                password,
                &options,
                emit,
            ),
        }
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

fn emit_edit_progress(app: &AppHandle, progress: crate::models::OperationProgress) {
    if let Err(error) = app.emit("edit-progress", progress) {
        eprintln!("Failed to emit edit-progress: {error}");
    }
}

#[command]
pub async fn delete_archive_entries_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    archive_path: String,
    paths: Vec<String>,
    options: Option<EditOptions>,
) -> Result<EditSummary, CommandError> {
    let state = registry
        .start_edit(&operation_id, &archive_path)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let edit_options = options.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        delete_entries(
            Path::new(&archive_path),
            &paths,
            &worker_operation_id,
            &cancelled,
            move |progress| emit_edit_progress(&progress_app, progress),
            edit_options.password.clone(),
            &edit_options,
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

#[command]
pub async fn rename_archive_entry_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    archive_path: String,
    from_path: String,
    to_path: String,
    options: Option<EditOptions>,
) -> Result<EditSummary, CommandError> {
    let state = registry
        .start_edit(&operation_id, &archive_path)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let edit_options = options.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        rename_entry(
            Path::new(&archive_path),
            &from_path,
            &to_path,
            &worker_operation_id,
            &cancelled,
            move |progress| emit_edit_progress(&progress_app, progress),
            edit_options.password.clone(),
            &edit_options,
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

#[command]
pub async fn create_archive_folder_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    archive_path: String,
    folder_path: String,
    options: Option<EditOptions>,
) -> Result<EditSummary, CommandError> {
    let state = registry
        .start_edit(&operation_id, &archive_path)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let edit_options = options.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        create_folder(
            Path::new(&archive_path),
            &folder_path,
            &worker_operation_id,
            &cancelled,
            move |progress| emit_edit_progress(&progress_app, progress),
            edit_options.password.clone(),
            &edit_options,
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

#[command]
pub async fn add_to_archive_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    archive_path: String,
    source_paths: Vec<String>,
    archive_parent: String,
    options: Option<EditOptions>,
) -> Result<EditSummary, CommandError> {
    let state = registry
        .start_edit(&operation_id, &archive_path)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let edit_options = options.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        add_paths(
            Path::new(&archive_path),
            &source_paths,
            &archive_parent,
            &worker_operation_id,
            &cancelled,
            move |progress| emit_edit_progress(&progress_app, progress),
            edit_options.password.clone(),
            &edit_options,
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

#[command]
pub async fn replace_archive_file_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    archive_path: String,
    entry_path: String,
    source_file: String,
    options: Option<EditOptions>,
) -> Result<EditSummary, CommandError> {
    let state = registry
        .start_edit(&operation_id, &archive_path)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let edit_options = options.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        replace_file(
            Path::new(&archive_path),
            &entry_path,
            Path::new(&source_file),
            &worker_operation_id,
            &cancelled,
            move |progress| emit_edit_progress(&progress_app, progress),
            edit_options.password.clone(),
            &edit_options,
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

/// Move archive entries into a destination folder (in-archive DnD).
#[command]
pub async fn move_archive_entries_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    archive_path: String,
    source_paths: Vec<String>,
    dest_folder: String,
    options: Option<EditOptions>,
) -> Result<EditSummary, CommandError> {
    let state = registry
        .start_edit(&operation_id, &archive_path)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let edit_options = options.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        move_entries(
            Path::new(&archive_path),
            &source_paths,
            &dest_folder,
            &worker_operation_id,
            &cancelled,
            move |progress| emit_edit_progress(&progress_app, progress),
            edit_options.password.clone(),
            &edit_options,
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

/// Full clean rewrite (ZIP reclaims orphan local data after fast logical delete).
#[command]
pub async fn compact_archive_command(
    app: tauri::AppHandle,
    registry: State<'_, OperationRegistry>,
    operation_id: String,
    archive_path: String,
    options: Option<EditOptions>,
) -> Result<EditSummary, CommandError> {
    let state = registry
        .start_edit(&operation_id, &archive_path)
        .map_err(|message| CommandError::new("operation_failed", message))?;
    let cancelled = state.cancelled.clone();
    let progress_app = app.clone();
    let worker_operation_id = operation_id.clone();
    let edit_options = options.unwrap_or_default();
    let result = tauri::async_runtime::spawn_blocking(move || {
        compact_archive(
            Path::new(&archive_path),
            &worker_operation_id,
            &cancelled,
            move |progress| emit_edit_progress(&progress_app, progress),
            edit_options.password.clone(),
            &edit_options,
        )
    })
    .await;
    registry.finish(&operation_id);
    result.map_err(|error| CommandError::new("worker_failed", error.to_string()))?
}

fn file_path_to_string(fp: tauri_plugin_dialog::FilePath) -> String {
    match fp {
        tauri_plugin_dialog::FilePath::Path(path) => path.to_string_lossy().into_owned(),
        tauri_plugin_dialog::FilePath::Url(url) => url.path().to_string(),
    }
}

#[command]
pub fn select_archive_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let file_path = app
        .dialog()
        .file()
        .add_filter(
            "Archives",
            &[
                "zip", "tar", "gz", "tgz", "bz2", "tbz2", "tbz", "xz", "txz", "7z", "rar",
            ],
        )
        .add_filter("ZIP", &["zip"])
        .add_filter("RAR", &["rar"])
        .add_filter(
            "TAR",
            &[
                "tar", "tar.gz", "tgz", "tar.bz2", "tbz2", "tbz", "tar.xz", "txz",
            ],
        )
        .add_filter("GZIP", &["gz"])
        .add_filter("BZIP2", &["bz2", "tbz2", "tbz"])
        .add_filter("XZ", &["xz", "txz"])
        .add_filter("7z", &["7z"])
        .blocking_pick_file();
    Ok(file_path.map(file_path_to_string))
}

#[command]
pub fn select_directory(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let dir_path = app.dialog().file().blocking_pick_folder();
    Ok(dir_path.map(file_path_to_string))
}

#[command]
pub fn select_save_archive(
    app: tauri::AppHandle,
    format: Option<CreateFormat>,
) -> Result<Option<String>, String> {
    let mut dialog = app.dialog().file();
    match format.unwrap_or(CreateFormat::Zip) {
        CreateFormat::Zip => {
            dialog = dialog.add_filter("ZIP Archive", &["zip"]);
        }
        CreateFormat::Tar => {
            dialog = dialog.add_filter("TAR Archive", &["tar"]);
        }
        CreateFormat::TarGz => {
            dialog = dialog
                .add_filter("TAR.GZ Archive", &["tar.gz", "tgz"])
                .add_filter("TGZ", &["tgz"]);
        }
        CreateFormat::TarBz2 => {
            dialog = dialog
                .add_filter("TAR.BZ2 Archive", &["tar.bz2", "tbz2"])
                .add_filter("TBZ2", &["tbz2"]);
        }
        CreateFormat::TarXz => {
            dialog = dialog
                .add_filter("TAR.XZ Archive", &["tar.xz", "txz"])
                .add_filter("TXZ", &["txz"]);
        }
        CreateFormat::SevenZ => {
            dialog = dialog.add_filter("7z Archive", &["7z"]);
        }
    }
    let file_path = dialog.blocking_save_file();
    Ok(file_path.map(file_path_to_string))
}

#[command]
pub fn select_multiple_files(app: tauri::AppHandle) -> Result<Option<Vec<String>>, String> {
    let file_paths = app.dialog().file().blocking_pick_files();
    Ok(file_paths.map(|paths| paths.into_iter().map(file_path_to_string).collect()))
}

/// Ensures a single leaf directory exists under an already-existing parent.
/// Does not create intermediate parents — parent must already resolve as a directory.
pub fn ensure_directory_path(path: &Path) -> Result<String, CommandError> {
    if path.as_os_str().is_empty() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination path is empty.",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        CommandError::new(
            "invalid_destination",
            "Destination has no parent directory.",
        )
    })?;
    if parent.as_os_str().is_empty() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination parent is invalid.",
        ));
    }

    // Reject link/reparse parents before following them via canonicalize.
    let parent_meta = fs::symlink_metadata(parent).map_err(|e| {
        CommandError::new(
            "invalid_destination",
            format!("Cannot resolve destination parent: {e}"),
        )
    })?;
    if is_link_or_reparse_point(&parent_meta) || !parent_meta.is_dir() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination parent is not a usable directory.",
        ));
    }

    let parent = parent.canonicalize().map_err(|e| {
        CommandError::new(
            "invalid_destination",
            format!("Cannot resolve destination parent: {e}"),
        )
    })?;
    if !parent.is_dir() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination parent is not a directory.",
        ));
    }
    let name = path.file_name().ok_or_else(|| {
        CommandError::new("invalid_destination", "Destination leaf name is missing.")
    })?;
    let candidate = parent.join(name);
    if candidate.exists() {
        let meta = fs::symlink_metadata(&candidate).map_err(|e| {
            CommandError::new(
                "invalid_destination",
                format!("Cannot inspect destination: {e}"),
            )
        })?;
        if is_link_or_reparse_point(&meta) || !meta.is_dir() {
            return Err(CommandError::new(
                "invalid_destination",
                "Destination exists and is not a usable directory.",
            ));
        }
    } else {
        fs::create_dir(&candidate).map_err(|e| {
            CommandError::new(
                "invalid_destination",
                format!("Cannot create destination directory: {e}"),
            )
        })?;
    }
    let canonical = candidate.canonicalize().map_err(|e| {
        CommandError::new(
            "invalid_destination",
            format!("Cannot resolve created destination: {e}"),
        )
    })?;
    if !canonical.starts_with(&parent) {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination escaped parent directory.",
        ));
    }
    Ok(canonical.to_string_lossy().into_owned())
}

#[command]
pub fn ensure_directory(path: String) -> Result<String, CommandError> {
    ensure_directory_path(Path::new(&path))
}

/// Opt-in Explorer associations status (HKCU). Always available; `supported` is false off Windows.
#[command]
pub fn get_file_association_status_command() -> FileAssociationStatus {
    get_file_association_status()
}

/// Register Archi as open handler for archive extensions (current user only).
#[command]
pub fn register_file_associations_command() -> Result<FileAssociationStatus, CommandError> {
    register_file_associations()
}

/// Remove Archi associations written by this app (current user only).
#[command]
pub fn unregister_file_associations_command() -> Result<FileAssociationStatus, CommandError> {
    unregister_file_associations()
}

static DRAG_SESSION_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DRAG_ICON_BYTES: &[u8] = include_bytes!("../icons/32x32.png");

struct StagingOperation {
    session_id: u64,
    archive_path: String,
    selected_paths: Vec<String>,
    result: std::sync::Arc<tokio::sync::Mutex<Option<Result<(std::path::PathBuf, Vec<std::path::PathBuf>), String>>>>,
    notify: std::sync::Arc<tokio::sync::Notify>,
}

static ACTIVE_STAGING: std::sync::Mutex<Option<StagingOperation>> = std::sync::Mutex::new(None);

/// Cancel any in-flight drag-out extraction or pending modal drag.
#[command]
pub fn cancel_drag_out() {
    DRAG_SESSION_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let old = {
        let mut lock = ACTIVE_STAGING.lock().unwrap();
        lock.take()
    };
    if let Some(op) = old {
        if let Ok(guard) = op.result.try_lock() {
            if let Some(Ok((ref temp_dir, _))) = *guard {
                let _ = std::fs::remove_dir_all(temp_dir);
            }
        }
    }
}

/// Pre-stages drag-out files in the background on pointerdown so they are ready immediately on drag.
#[command]
pub async fn prepare_drag_out(
    archive_path: String,
    selected_paths: Vec<String>,
    password: Option<String>,
) -> Result<(), CommandError> {
    let session_id = DRAG_SESSION_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let result_holder = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());

    {
        let mut lock = ACTIVE_STAGING.lock().unwrap();
        if let Some(old) = lock.take() {
            if let Ok(guard) = old.result.try_lock() {
                if let Some(Ok((ref temp_dir, _))) = *guard {
                    let _ = std::fs::remove_dir_all(temp_dir);
                }
            }
        }
        *lock = Some(StagingOperation {
            session_id,
            archive_path: archive_path.clone(),
            selected_paths: selected_paths.clone(),
            result: result_holder.clone(),
            notify: notify.clone(),
        });
    }

    let archive_path_clone = archive_path.clone();
    let selected_paths_clone = selected_paths.clone();

    let res = tauri::async_runtime::spawn_blocking(move || {
        crate::drag_out::stage_drag_files(Path::new(&archive_path_clone), &selected_paths_clone, password)
    })
    .await
    .map_err(|e| CommandError::new("worker_failed", e.to_string()))?;

    if DRAG_SESSION_COUNTER.load(std::sync::atomic::Ordering::SeqCst) == session_id {
        match res {
            Ok(tuple) => {
                let mut guard = result_holder.lock().await;
                *guard = Some(Ok(tuple));
                notify.notify_waiters();
            }
            Err(e) => {
                let mut guard = result_holder.lock().await;
                *guard = Some(Err(e.message));
                notify.notify_waiters();
            }
        }
    } else {
        if let Ok((temp_dir, _)) = res {
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
    }

    Ok(())
}

/// Extract selected archive entries to temporary staging and start native drag operation on main thread.
#[command]
pub async fn start_drag_out(
    app: AppHandle,
    window: tauri::Window,
    archive_path: String,
    selected_paths: Vec<String>,
    password: Option<String>,
) -> Result<(), CommandError> {
    let staging_op = {
        let lock = ACTIVE_STAGING.lock().unwrap();
        if let Some(ref op) = *lock {
            if op.archive_path == archive_path && op.selected_paths == selected_paths {
                Some((op.session_id, op.result.clone(), op.notify.clone()))
            } else {
                None
            }
        } else {
            None
        }
    };

    let (temp_dir, disk_paths, session_id) = if let Some((sid, result_holder, notify)) = staging_op {
        loop {
            if DRAG_SESSION_COUNTER.load(std::sync::atomic::Ordering::SeqCst) != sid {
                return Ok(());
            }
            {
                let guard = result_holder.lock().await;
                if let Some(ref res) = *guard {
                    match res {
                        Ok((t, d)) => {
                            let res_tuple = (t.clone(), d.clone(), sid);
                            let mut lock = ACTIVE_STAGING.lock().unwrap();
                            let _ = lock.take();
                            break res_tuple;
                        }
                        Err(msg) => return Err(CommandError::new("drag_staging_failed", msg.clone())),
                    }
                }
            }
            tokio::select! {
                _ = notify.notified() => {},
                _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
            }
        }
    } else {
        let session_id = DRAG_SESSION_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let archive_path_clone = archive_path.clone();
        let selected_paths_clone = selected_paths.clone();
        let (t, d) = tauri::async_runtime::spawn_blocking(move || {
            crate::drag_out::stage_drag_files(Path::new(&archive_path_clone), &selected_paths_clone, password)
        })
        .await
        .map_err(|e| CommandError::new("worker_failed", e.to_string()))??;
        (t, d, session_id)
    };

    // Abort if cancelled or a newer drag operation was started while extracting:
    if DRAG_SESSION_COUNTER.load(std::sync::atomic::Ordering::SeqCst) != session_id {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Ok(());
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        if DRAG_SESSION_COUNTER.load(std::sync::atomic::Ordering::SeqCst) != session_id {
            let _ = tx.send(Ok(()));
            return;
        }

        let r = drag::start_drag(
            &window,
            drag::DragItem::Files(disk_paths),
            drag::Image::Raw(DRAG_ICON_BYTES.to_vec()),
            |_res, _pos| {},
            drag::Options {
                mode: drag::DragMode::Copy,
                skip_animatation_on_cancel_or_failure: false,
            },
        );
        let _ = tx.send(r);
    })
    .map_err(|e| CommandError::new("main_thread_error", e.to_string()))?;

    let res = rx
        .recv()
        .map_err(|e| CommandError::new("drag_recv_error", e.to_string()))?;

    crate::drag_out::register_temp_dir_for_cleanup(temp_dir);

    res.map_err(|e| CommandError::new("drag_failed", e.to_string()))
}
