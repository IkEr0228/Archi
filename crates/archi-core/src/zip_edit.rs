use crate::create_common::{
    cleanup_temp, create_temporary_archive as create_temporary_edit_archive, publish_temp_archive,
};
use crate::edit_plan::{
    normalize_and_validate, plan_add_paths, plan_create_folder, plan_delete, plan_move,
    plan_rename, plan_replace_file, RebuildMember, SourceMember,
};
use crate::edit_sources::source_metadata;
use crate::extraction::normalize_entry_name;
use crate::models::{CommandError, EditOptions, EditStrategyPref, OperationProgress};

pub use crate::models::EditSummary;
use crate::security::validate_entry_path;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::io_perf::{ProgressGate, IO_BUFFER_SIZE as BUFFER_SIZE};

/// Prefer append for Auto / PreferFast / unspecified; PreferCompact always rebuilds.
fn should_try_append(options: &EditOptions) -> bool {
    !matches!(options.strategy, Some(EditStrategyPref::PreferCompact))
}

/// Whether to attempt ZIP logical delete (CD rewrite) for this strategy and delete set size.
///
/// - PreferCompact: never
/// - PreferFast: always try
/// - Auto / None: logical when `deleted/total ≤ 0.25` or `deleted ≤ 64`; else rebuild
fn should_try_logical_delete(options: &EditOptions, deleted: usize, total: usize) -> bool {
    match options.strategy {
        Some(EditStrategyPref::PreferCompact) => false,
        Some(EditStrategyPref::PreferFast) => true,
        Some(EditStrategyPref::Auto) | None => {
            if deleted == 0 || total == 0 {
                return false;
            }
            let fraction = deleted as f64 / total as f64;
            fraction <= 0.25 || deleted <= 64
        }
    }
}

/// Normalize delete selection the same way as `plan_delete` (for CD filtering).
fn normalize_delete_selection(paths: &[String]) -> Result<Vec<String>, CommandError> {
    if paths.is_empty() {
        return Err(edit_error(
            "invalid_selection",
            "No archive paths specified for delete.",
        ));
    }
    let mut selected = Vec::with_capacity(paths.len());
    for raw in paths {
        selected.push(normalize_and_validate(raw)?);
    }
    Ok(selected)
}

/// New directory / file members only (no Copy) — what append actually writes.
fn new_members_only(planned: &[RebuildMember]) -> Vec<RebuildMember> {
    planned
        .iter()
        .filter(|m| {
            matches!(
                m,
                RebuildMember::NewDirectory { .. } | RebuildMember::NewFile { .. }
            )
        })
        .cloned()
        .collect()
}

fn edit_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn cancelled_error() -> CommandError {
    edit_error("cancelled", "Archive edit was cancelled.")
}

fn progress_percentage(processed: u64, total: u64) -> f32 {
    if total == 0 {
        100.0
    } else {
        ((processed as f64 * 100.0 / total as f64).min(100.0)) as f32
    }
}

/// Byte-for-byte copy of `zip_path` to a sibling temp file, opened for read+write+seek (append).
fn create_temporary_archive_copy(zip_path: &Path) -> Result<(PathBuf, File), CommandError> {
    let (temp_path, temp_file) = create_temporary_edit_archive(zip_path)?;
    drop(temp_file);
    if let Err(error) = fs::copy(zip_path, &temp_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(edit_error(
            "temp_create_failed",
            format!("Cannot copy archive for append: {error}"),
        ));
    }
    match OpenOptions::new().read(true).write(true).open(&temp_path) {
        Ok(file) => Ok((temp_path, file)),
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(edit_error(
                "temp_create_failed",
                format!("Cannot open temporary archive for append: {error}"),
            ))
        }
    }
}

fn open_source_members(
    zip_path: &Path,
) -> Result<(ZipArchive<File>, Vec<SourceMember>), CommandError> {
    let file = File::open(zip_path).map_err(|error| {
        edit_error(
            "invalid_archive",
            format!("Failed to open archive: {error}"),
        )
    })?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        edit_error(
            "invalid_archive",
            format!("Cannot open or read ZIP structure: {error}"),
        )
    })?;

    let mut members = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(|error| {
            edit_error(
                "invalid_archive",
                format!("Failed to read zip entry: {error}"),
            )
        })?;
        if entry.encrypted() {
            return Err(edit_error(
                "unsupported_encrypted_edit",
                "Editing encrypted ZIP is not supported. Original archive was not changed.",
            ));
        }
        let raw_name = entry.name().to_string();
        validate_entry_path(&raw_name).map_err(|message| CommandError {
            code: "invalid_entry".into(),
            message,
            path: Some(raw_name.clone()),
        })?;
        let path = normalize_entry_name(&raw_name);
        if path.is_empty() {
            return Err(CommandError {
                code: "invalid_entry".into(),
                message: "Archive entry path is empty or malformed.".into(),
                path: Some(raw_name),
            });
        }
        let is_dir = entry.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\');
        members.push(SourceMember {
            index,
            path,
            is_dir,
        });
    }

    Ok((archive, members))
}

#[cfg(windows)]
fn open_source_file(path: &Path) -> io::Result<crate::windows_fs::PinnedFile> {
    crate::windows_fs::open_source_file(path)
}

#[cfg(not(windows))]
fn open_source_file(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn deflated_file_options() -> SimpleFileOptions {
    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)
}

fn zip_output_name(path: &str, is_dir: bool) -> String {
    if is_dir {
        format!("{path}/")
    } else {
        path.to_string()
    }
}

fn write_new_file_entry(
    zip: &mut ZipWriter<impl Write + Seek>,
    archive_path: &str,
    source: &Path,
    cancelled: &AtomicBool,
) -> Result<(), CommandError> {
    // Re-check links immediately before open (TOCTOU).
    let metadata = source_metadata(source)?;
    if !metadata.is_file() {
        return Err(edit_error(
            "invalid_source",
            format!("Source is not a regular file: {}", source.display()),
        ));
    }

    let mut reader = open_source_file(source).map_err(|error| {
        edit_error(
            "source_read",
            format!("Cannot open source {}: {error}", source.display()),
        )
    })?;
    zip.start_file(archive_path, deflated_file_options())
        .map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot start ZIP file {archive_path}: {error}"),
            )
        })?;

    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        let read = reader.read(&mut buffer).map_err(|error| {
            edit_error(
                "source_read",
                format!("Cannot read source {}: {error}", source.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        zip.write_all(&buffer[..read]).map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot write ZIP data for {archive_path}: {error}"),
            )
        })?;
    }
    Ok(())
}

fn rebuild_archive(
    zip_path: &Path,
    mut archive: ZipArchive<File>,
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let total_files = planned.len() as u64;
    let (temp_path, temp_file) = create_temporary_edit_archive(zip_path)?;

    let result = (|| -> Result<EditSummary, CommandError> {
        let mut zip = ZipWriter::new(BufWriter::with_capacity(BUFFER_SIZE, temp_file));
        let mut processed = 0_u64;
        let mut progress_gate = ProgressGate::new();

        for member in planned {
            if cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let current_file = member.out_path().to_string();
            // First member always; mid-members at most every PROGRESS_INTERVAL; final 100% outside loop.
            if progress_gate.should_emit() {
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files: processed,
                    total_files,
                    current_file: current_file.clone(),
                    percentage: progress_percentage(processed, total_files),
                    phase: Some("rebuild".into()),
                });
            }

            match member {
                RebuildMember::Copy {
                    index,
                    out_path,
                    is_dir,
                } => {
                    let entry = archive.by_index(*index).map_err(|error| {
                        edit_error(
                            "invalid_archive",
                            format!("Failed to read zip entry: {error}"),
                        )
                    })?;
                    let out_name = zip_output_name(out_path, *is_dir);

                    if entry.is_dir() || *is_dir {
                        let options = SimpleFileOptions::default();
                        zip.add_directory(&out_name, options).map_err(|error| {
                            edit_error(
                                "write_failed",
                                format!("Cannot write directory entry: {error}"),
                            )
                        })?;
                    } else {
                        zip.raw_copy_file_rename(entry, out_name).map_err(|error| {
                            edit_error(
                                "write_failed",
                                format!("Cannot copy archive entry: {error}"),
                            )
                        })?;
                    }
                }
                RebuildMember::NewDirectory { path } => {
                    let out_name = zip_output_name(path, true);
                    zip.add_directory(&out_name, SimpleFileOptions::default())
                        .map_err(|error| {
                            edit_error(
                                "write_failed",
                                format!("Cannot write directory entry: {error}"),
                            )
                        })?;
                }
                RebuildMember::NewFile { path, source } => {
                    write_new_file_entry(&mut zip, path, source, cancelled)?;
                }
            }

            processed += 1;
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let mut temp_file = zip.finish().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot finalize temporary ZIP: {error}"),
            )
        })?;
        temp_file
            .flush()
            .map_err(|e| edit_error("write_failed", format!("Cannot flush temporary ZIP: {e}")))?;
        temp_file.get_ref().sync_all().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot sync temporary ZIP: {error}"),
            )
        })?;
        drop(temp_file);
        // Release the source ZIP handle before replacing the file on Windows.
        drop(archive);

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        publish_temp_archive(&temp_path, zip_path, true).map_err(|error| {
            edit_error(
                "finalize_failed",
                format!("Cannot replace archive with edited copy: {error}"),
            )
        })?;

        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: zip_path.to_string_lossy().into_owned(),
            members_written: processed,
            strategy_used: Some("rebuild".into()),
        })
    })();

    match result {
        Ok(summary) => {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: summary.members_written,
                total_files: summary.members_written,
                current_file: "Completed".into(),
                percentage: 100.0,
                phase: Some("rebuild".into()),
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}

/// Append only new directory/file members onto a byte-copy of the archive via `ZipWriter::new_append`.
///
/// Does not rewrite existing central-directory entries; fails (caller may fall back to rebuild)
/// on unsupported ZIP features or I/O errors. Never mutates `zip_path` until publish succeeds.
fn append_to_archive(
    zip_path: &Path,
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let to_add = new_members_only(planned);
    if to_add.is_empty() {
        return Err(edit_error(
            "invalid_operation",
            "Append strategy requires at least one new member.",
        ));
    }

    let total_files = to_add.len() as u64;
    let (temp_path, temp_file) = create_temporary_archive_copy(zip_path)?;

    let result = (|| -> Result<EditSummary, CommandError> {
        let mut zip = ZipWriter::new_append(temp_file).map_err(|error| {
            edit_error(
                "append_unsupported",
                format!("Cannot open ZIP for append: {error}"),
            )
        })?;

        let mut processed = 0_u64;
        let mut progress_gate = ProgressGate::new();

        for member in &to_add {
            if cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let current_file = member.out_path().to_string();
            if progress_gate.should_emit() {
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files: processed,
                    total_files,
                    current_file: current_file.clone(),
                    percentage: progress_percentage(processed, total_files),
                    phase: Some("append".into()),
                });
            }

            match member {
                RebuildMember::NewDirectory { path } => {
                    let out_name = zip_output_name(path, true);
                    zip.add_directory(&out_name, SimpleFileOptions::default())
                        .map_err(|error| {
                            edit_error(
                                "write_failed",
                                format!("Cannot write directory entry: {error}"),
                            )
                        })?;
                }
                RebuildMember::NewFile { path, source } => {
                    write_new_file_entry(&mut zip, path, source, cancelled)?;
                }
                RebuildMember::Copy { .. } => unreachable!("filtered by new_members_only"),
            }

            processed += 1;
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let temp_file = zip.finish().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot finalize temporary ZIP: {error}"),
            )
        })?;
        temp_file.sync_all().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot sync temporary ZIP: {error}"),
            )
        })?;
        drop(temp_file);

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        publish_temp_archive(&temp_path, zip_path, true).map_err(|error| {
            edit_error(
                "finalize_failed",
                format!("Cannot replace archive with edited copy: {error}"),
            )
        })?;

        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: zip_path.to_string_lossy().into_owned(),
            members_written: processed,
            strategy_used: Some("append".into()),
        })
    })();

    match result {
        Ok(summary) => {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: summary.members_written,
                total_files: summary.members_written,
                current_file: "Completed".into(),
                percentage: 100.0,
                phase: Some("append".into()),
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}

/// Try ZIP append when strategy allows; on any non-cancel append failure, full rebuild.
fn apply_add_like_edit(
    zip_path: &Path,
    archive: ZipArchive<File>,
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    if should_try_append(options) {
        drop(archive);
        match append_to_archive(zip_path, planned, operation_id, cancelled, &mut emit) {
            Ok(summary) => return Ok(summary),
            Err(error) if error.code == "cancelled" => return Err(error),
            Err(_) => {
                // Fall through to full rebuild (unsupported ZIP features, open/write errors).
            }
        }
        let (archive, _) = open_source_members(zip_path)?;
        return rebuild_archive(zip_path, archive, planned, operation_id, cancelled, emit);
    }
    rebuild_archive(zip_path, archive, planned, operation_id, cancelled, emit)
}

/// Logical delete: copy archive to temp, rewrite central directory (orphan local data), publish.
///
/// Never mutates `zip_path` until publish. On cancel, cleans up temp and does not rebuild.
fn logical_delete_archive(
    zip_path: &Path,
    selected: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let (temp_path, mut temp_file) = create_temporary_archive_copy(zip_path)?;

    let result = (|| -> Result<EditSummary, CommandError> {
        emit(OperationProgress {
            operation_id: operation_id.into(),
            extracted_files: 0,
            total_files: 1,
            current_file: "logical_delete".into(),
            percentage: 0.0,
            phase: Some("logical_delete".into()),
        });

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let kept = crate::zip_cd::logical_delete_on_file(&mut temp_file, selected, cancelled)?;
        drop(temp_file);

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        // Validate before replacing the original.
        {
            let validate = File::open(&temp_path).map_err(|error| {
                edit_error(
                    "write_failed",
                    format!("Cannot reopen temporary ZIP for validation: {error}"),
                )
            })?;
            ZipArchive::new(validate).map_err(|error| {
                edit_error(
                    "write_failed",
                    format!("Logical delete produced an invalid ZIP: {error}"),
                )
            })?;
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        publish_temp_archive(&temp_path, zip_path, true).map_err(|error| {
            edit_error(
                "finalize_failed",
                format!("Cannot replace archive with edited copy: {error}"),
            )
        })?;

        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: zip_path.to_string_lossy().into_owned(),
            members_written: kept,
            strategy_used: Some("logical_delete".into()),
        })
    })();

    match result {
        Ok(summary) => {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: summary.members_written,
                total_files: summary.members_written.max(1),
                current_file: "Completed".into(),
                percentage: 100.0,
                phase: Some("logical_delete".into()),
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}

fn require_zip_file(zip_path: &Path) -> Result<(), CommandError> {
    if !zip_path.is_file() {
        return Err(edit_error(
            "not_found",
            format!("Archive not found: {}", zip_path.display()),
        ));
    }
    Ok(())
}

/// Deletes archive entries matching `paths` (exact or directory prefix).
///
/// Strategy:
/// - PreferCompact: full rebuild
/// - PreferFast: try logical delete (CD rewrite); non-cancel failure → rebuild
/// - Auto / default: logical when `deleted/total ≤ 0.25` or `deleted ≤ 64`; else rebuild
///
/// Cancel during logical delete cleans up temp and does **not** fall back to rebuild.
pub fn delete_entries(
    zip_path: &Path,
    paths: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (archive, members) = open_source_members(zip_path)?;
    let planned = plan_delete(&members, paths)?;
    let total = members.len();
    let kept = planned.len();
    let deleted = total.saturating_sub(kept);

    if should_try_logical_delete(options, deleted, total) {
        drop(archive);
        let selected = normalize_delete_selection(paths)?;
        match logical_delete_archive(zip_path, &selected, operation_id, cancelled, &mut emit) {
            Ok(summary) => return Ok(summary),
            Err(error) if error.code == "cancelled" => return Err(error),
            Err(_) => {
                // PreferFast / Auto: fall through to full rebuild on non-cancel failure.
            }
        }
        let (archive, _) = open_source_members(zip_path)?;
        return rebuild_archive(zip_path, archive, &planned, operation_id, cancelled, emit);
    }

    rebuild_archive(zip_path, archive, &planned, operation_id, cancelled, emit)
}

/// Renames a file entry or directory prefix inside a ZIP archive.
pub fn rename_entry(
    zip_path: &Path,
    from: &str,
    to: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (archive, members) = open_source_members(zip_path)?;
    let planned = plan_rename(&members, from, to)?;
    rebuild_archive(zip_path, archive, &planned, operation_id, cancelled, emit)
}

/// Moves entries into `dest_folder` (leaf names preserved). One ZIP rebuild.
pub fn move_entries(
    zip_path: &Path,
    sources: &[String],
    dest_folder: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (archive, members) = open_source_members(zip_path)?;
    let planned = plan_move(&members, sources, dest_folder)?;
    rebuild_archive(zip_path, archive, &planned, operation_id, cancelled, emit)
}

/// Creates an empty directory entry in the ZIP archive.
///
/// Uses append when strategy allows (`Auto` / `PreferFast` / default); falls back to rebuild.
pub fn create_folder(
    zip_path: &Path,
    folder_path: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (archive, members) = open_source_members(zip_path)?;
    let planned = plan_create_folder(&members, folder_path)?;
    apply_add_like_edit(
        zip_path,
        archive,
        &planned,
        operation_id,
        cancelled,
        emit,
        options,
    )
}

/// Adds files and directories from disk into the ZIP under `archive_parent`.
///
/// Directory sources include their root folder name (like create with include_root).
/// Rejects symlink/reparse sources and existing archive targets.
/// Uses append when strategy allows (`Auto` / `PreferFast` / default); falls back to rebuild.
pub fn add_paths(
    zip_path: &Path,
    source_paths: &[String],
    archive_parent: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (archive, members) = open_source_members(zip_path)?;
    let planned = plan_add_paths(&members, source_paths, archive_parent, zip_path, cancelled)?;
    apply_add_like_edit(
        zip_path,
        archive,
        &planned,
        operation_id,
        cancelled,
        emit,
        options,
    )
}

/// Replaces an existing file entry's content from a disk file (same archive path).
pub fn replace_file(
    zip_path: &Path,
    entry_path: &str,
    source_file: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (archive, members) = open_source_members(zip_path)?;
    let planned = plan_replace_file(&members, entry_path, source_file)?;
    rebuild_archive(zip_path, archive, &planned, operation_id, cancelled, emit)
}

/// Full clean rebuild of every CD-visible member (reclaims orphan local data after logical delete).
pub fn compact_archive(
    zip_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (archive, members) = open_source_members(zip_path)?;
    let planned: Vec<RebuildMember> = members
        .iter()
        .map(|member| RebuildMember::Copy {
            index: member.index,
            out_path: member.path.clone(),
            is_dir: member.is_dir,
        })
        .collect();
    let mut summary = rebuild_archive(zip_path, archive, &planned, operation_id, cancelled, emit)?;
    summary.strategy_used = Some("compact".into());
    Ok(summary)
}
