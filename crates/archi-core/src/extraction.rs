use crate::bzip2_format::extract_bzip2;
#[cfg(windows)]
use crate::conflict::unique_renamed_path;
use crate::format_detect::{detect_format, ArchiveFormat};
use crate::gzip_format::extract_gzip;
use crate::models::{CommandError, ConflictDecision, OperationProgress, OperationSummary};
use crate::rar_format::extract_rar;
use crate::security::{
    destination_path_error_code, is_link_or_reparse_point, safe_destination_path_under_canonical,
};
use crate::sevenz_format::extract_sevenz;
use crate::tar_format::{extract_tar, extract_tar_bz2, extract_tar_gz, extract_tar_xz};
#[cfg(windows)]
use crate::windows_fs::{cleanup_created as cleanup_windows_created, Directory, LeafProbe};
use crate::xz_format::extract_xz;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::{Read, Write};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use zip::ZipArchive;

use crate::io_perf::{IO_BUFFER_SIZE as BUFFER_SIZE, PROGRESS_INTERVAL};

/// Check cancel during ZIP central-directory plan walk every this many entries.
const PLAN_CANCEL_CHECK_INTERVAL: usize = 256;

/// Decides how to handle an existing regular file at the extract destination.
pub trait ConflictResolver: Send {
    fn resolve_file_exists(
        &self,
        operation_id: &str,
        entry_path: &str,
        dest_path: &Path,
    ) -> Result<ConflictDecision, CommandError>;
}

/// Hard-fail resolver used by production until a UI-backed resolver is wired,
/// and by tests that expect legacy conflict errors.
pub struct FailOnConflict;

impl ConflictResolver for FailOnConflict {
    fn resolve_file_exists(
        &self,
        _operation_id: &str,
        _entry_path: &str,
        _dest_path: &Path,
    ) -> Result<ConflictDecision, CommandError> {
        Err(CommandError::new(
            "conflict",
            "Destination entry already exists.",
        ))
    }
}

/// Headless/CLI conflict resolver that overwrites existing files.
pub struct AutoOverwriteConflictResolver;

impl ConflictResolver for AutoOverwriteConflictResolver {
    fn resolve_file_exists(
        &self,
        _operation_id: &str,
        _entry_path: &str,
        _dest_path: &Path,
    ) -> Result<ConflictDecision, CommandError> {
        Ok(ConflictDecision::Overwrite)
    }
}

/// Test helper that returns scripted decisions in order for each file conflict.
pub struct ScriptedConflictResolver {
    pub decisions: Mutex<VecDeque<ConflictDecision>>,
}

impl ScriptedConflictResolver {
    pub fn new(decisions: impl IntoIterator<Item = ConflictDecision>) -> Self {
        Self {
            decisions: Mutex::new(decisions.into_iter().collect()),
        }
    }
}

impl ConflictResolver for ScriptedConflictResolver {
    fn resolve_file_exists(
        &self,
        _operation_id: &str,
        _entry_path: &str,
        _dest_path: &Path,
    ) -> Result<ConflictDecision, CommandError> {
        self.decisions
            .lock()
            .map_err(|_| {
                CommandError::new("operation_failed", "Conflict decision lock was poisoned.")
            })?
            .pop_front()
            .ok_or_else(|| {
                CommandError::new(
                    "conflict",
                    "No scripted conflict decision remaining for existing destination file.",
                )
            })
    }
}

struct PlannedEntry {
    index: usize,
    name: String,
    destination: PathBuf,
    is_directory: bool,
}

fn extraction_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

pub fn normalize_entry_name(name: &str) -> String {
    name.replace('\\', "/").trim_matches('/').to_string()
}

/// True if `entry` is exactly `sel` or a path under `sel/` (folder selection).
/// Avoids allocating `sel + "/"` on every comparison.
#[inline]
fn entry_matches_sel(entry: &str, sel: &str) -> bool {
    if sel.is_empty() {
        return false;
    }
    if entry == sel {
        return true;
    }
    entry.len() > sel.len()
        && entry.as_bytes().get(sel.len()) == Some(&b'/')
        && entry.starts_with(sel)
}

/// Pre-normalized selection for O(path depth) membership without per-call allocations.
#[derive(Debug, Clone)]
pub struct SelectionIndex {
    exact: HashSet<String>,
}

impl SelectionIndex {
    pub fn from_selected(selected: &[String]) -> Result<Self, CommandError> {
        let mut exact = HashSet::with_capacity(selected.len());
        for raw in selected {
            let sel = normalize_entry_name(raw);
            if sel.is_empty() {
                return Err(extraction_error(
                    "invalid_selection",
                    "Selection contains an empty path.",
                ));
            }
            exact.insert(sel);
        }
        Ok(Self { exact })
    }

    #[inline]
    pub fn includes_normalized(&self, entry: &str) -> bool {
        if self.exact.contains(entry) {
            return true;
        }
        entry
            .match_indices('/')
            .any(|(offset, _)| self.exact.contains(&entry[..offset]))
    }

    pub fn includes_entry(&self, entry_name: &str) -> bool {
        let entry = normalize_entry_name(entry_name);
        self.includes_normalized(&entry)
    }
}

pub fn selection_includes_entry(entry_name: &str, selected: &[String]) -> bool {
    let entry = normalize_entry_name(entry_name);
    selected.iter().any(|raw| {
        let sel = normalize_entry_name(raw);
        entry_matches_sel(&entry, &sel)
    })
}

pub fn validate_selection(
    selected: &[String],
    archive_names: &[String],
) -> Result<(), CommandError> {
    let index = SelectionIndex::from_selected(selected)?;
    let names: std::collections::BTreeSet<_> = archive_names
        .iter()
        .map(|n| normalize_entry_name(n))
        .collect();
    // Normalize archive names once, then exact/prefix lookup instead of S full scans.
    for sel in &index.exact {
        let prefix = format!("{sel}/");
        let ok = names.contains(sel)
            || names
                .range(prefix.clone()..)
                .next()
                .is_some_and(|entry| entry.starts_with(&prefix));
        if !ok {
            return Err(extraction_error(
                "invalid_selection",
                format!("Selected path is not in the archive: {sel}"),
            ));
        }
    }
    Ok(())
}

fn is_zip_symlink(mode: Option<u32>) -> bool {
    mode.is_some_and(|mode| mode & 0o170000 == 0o120000)
}

fn cancelled_error() -> CommandError {
    extraction_error("cancelled", "Extraction was cancelled.")
}

fn transaction_error(message: impl Into<String>) -> CommandError {
    extraction_error("transaction_failed", message)
}

fn transaction_backup_path(destination: &Path) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    destination.with_file_name(format!(".archi-transaction-{}-{nonce}", std::process::id()))
}

fn copy_transaction_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if is_link_or_reparse_point(&metadata) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Destination contains a symbolic link or reparse point.",
            ));
        }
        if metadata.is_dir() {
            copy_transaction_tree(&source_path, &target_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &target_path)?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Destination contains an unsupported file type.",
            ));
        }
    }
    Ok(())
}

fn remove_transaction_tree(root: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() && !is_link_or_reparse_point(&metadata) {
            fs::remove_dir_all(path)?;
        } else if metadata.is_dir() {
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn restore_transaction_tree(backup: &Path, destination: &Path) -> std::io::Result<()> {
    remove_transaction_tree(destination)?;
    for entry in fs::read_dir(backup)? {
        let entry = entry?;
        let source = entry.path();
        let target = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source)?;
        if metadata.is_dir() {
            copy_transaction_tree(&source, &target)?;
        } else {
            fs::copy(source, target)?;
        }
    }
    Ok(())
}

/// Run extraction with rollback-on-error semantics across all archive codecs.
///
/// This protects against partial files, replacements, and created directories
/// when decoding, validation, cancellation, or conflict handling fails. It is
/// rollback-safe, not crash-safe: power loss and external concurrent mutation
/// remain outside this transaction boundary.
pub fn extract_transactional(
    archive_path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    password: Option<String>,
    conflict_resolver: &dyn ConflictResolver,
    emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }
    if !destination.is_dir() {
        return extract_any(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            password,
            conflict_resolver,
            emit,
        );
    }
    let destination = destination.canonicalize().map_err(|error| {
        transaction_error(format!("Cannot resolve extraction destination: {error}"))
    })?;
    let backup = transaction_backup_path(&destination);
    copy_transaction_tree(&destination, &backup).map_err(|error| {
        let _ = fs::remove_dir_all(&backup);
        transaction_error(format!("Cannot create extraction transaction: {error}"))
    })?;

    let result = extract_any(
        archive_path,
        &destination,
        operation_id,
        cancelled,
        selected_paths,
        password,
        conflict_resolver,
        emit,
    );
    match result {
        Ok(summary) => {
            if let Err(error) = fs::remove_dir_all(&backup) {
                let mut transaction =
                    transaction_error(format!("Cannot finalize extraction transaction: {error}"));
                if let Err(restore_error) = restore_transaction_tree(&backup, &destination) {
                    transaction
                        .message
                        .push_str(&format!(" Rollback failed: {restore_error}."));
                }
                let _ = fs::remove_dir_all(&backup);
                return Err(transaction);
            }
            Ok(summary)
        }
        Err(mut error) => {
            if let Err(restore_error) = restore_transaction_tree(&backup, &destination) {
                error
                    .message
                    .push_str(&format!(" Transaction rollback failed: {restore_error}."));
                error.code = "transaction_failed".into();
            }
            if let Err(cleanup_error) = fs::remove_dir_all(&backup) {
                error
                    .message
                    .push_str(&format!(" Transaction cleanup failed: {cleanup_error}."));
                error.code = "transaction_failed".into();
            }
            Err(error)
        }
    }
}

/// Plan-phase existence rules for a destination path that already exists.
fn plan_existing_destination(
    is_directory_entry: bool,
    metadata: &fs::Metadata,
) -> Result<(), CommandError> {
    if is_link_or_reparse_point(metadata) {
        return Err(extraction_error(
            "unsafe_destination",
            "Destination entry is a symbolic link.",
        ));
    }
    if is_directory_entry && metadata.is_dir() {
        // Existing directory for a directory entry is fine.
        return Ok(());
    }
    if !is_directory_entry && !metadata.is_dir() {
        // Existing regular file for a file entry: defer to write-time resolver.
        return Ok(());
    }
    // Directory in the way of a file, or file in the way of a directory.
    Err(extraction_error(
        "conflict",
        "Destination entry already exists.",
    ))
}

/// Write-time handling when a regular file already occupies the planned leaf.
/// Returns `Some(final_wide_name)` to write, or `None` if the entry was skipped.
///
/// Overwrite authorizes replacement only after decoding has succeeded.
#[cfg(windows)]
fn resolve_existing_file_dest(
    operation_id: &str,
    plan: &PlannedEntry,
    leaf_name: &[u16],
    conflict_resolver: &dyn ConflictResolver,
    skipped_files: &mut u64,
) -> Result<Option<Vec<u16>>, CommandError> {
    let decision =
        conflict_resolver.resolve_file_exists(operation_id, &plan.name, &plan.destination)?;

    match decision {
        ConflictDecision::Overwrite => Ok(Some(leaf_name.to_vec())),
        ConflictDecision::Skip => {
            *skipped_files += 1;
            Ok(None)
        }
        ConflictDecision::Rename => {
            let parent_path = plan.destination.parent().ok_or_else(|| {
                extraction_error(
                    "unsafe_destination",
                    "Archive entry has no destination parent.",
                )
            })?;
            let file_name = plan
                .destination
                .file_name()
                .ok_or_else(|| {
                    extraction_error(
                        "unsafe_destination",
                        "Archive entry has no destination name.",
                    )
                })?
                .to_string_lossy();
            let renamed = unique_renamed_path(parent_path, &file_name)?;
            // Parent is already under destination; only the leaf name changes.
            let renamed_name = renamed.file_name().ok_or_else(|| {
                extraction_error(
                    "unsafe_destination",
                    "Renamed destination has no file name.",
                )
            })?;
            Ok(Some(renamed_name.encode_wide().collect()))
        }
        ConflictDecision::Cancel => Err(cancelled_error()),
    }
}

#[cfg(windows)]
fn extract_windows(
    archive: &mut ZipArchive<fs::File>,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    password: Option<&str>,
    conflict_resolver: &dyn ConflictResolver,
    emit: &mut impl FnMut(OperationProgress),
    plans: &[PlannedEntry],
) -> Result<OperationSummary, CommandError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }
    let root = Directory::open_root(destination).map_err(|error| {
        extraction_error(
            "unsafe_destination",
            format!("Cannot securely open destination: {error}"),
        )
    })?;
    let total_files = plans.len() as u64;
    let mut created = Vec::new();
    let mut dir_cache = ahash::AHashMap::new();
    let mut extracted_files = 0_u64;
    let mut skipped_files = 0_u64;
    let mut last_progress = Instant::now() - PROGRESS_INTERVAL;
    let result = (|| -> Result<OperationSummary, CommandError> {
        for plan in plans {
            if cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }
            if last_progress.elapsed() >= PROGRESS_INTERVAL {
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files,
                    total_files,
                    current_file: plan.name.clone(),
                    percentage: if total_files == 0 {
                        100.0
                    } else {
                        extracted_files as f32 * 100.0 / total_files as f32
                    },
                    phase: None,
                });
                last_progress = Instant::now();
            }

            if plan.is_directory {
                // Cache every path segment so later files under this directory
                // reuse the same handles (avoids STATUS_SHARING_VIOLATION when
                // the ZIP lists both "folder/" and "folder/file").
                root.ensure_path(destination, &plan.destination, &mut created, &mut dir_cache)
                    .map_err(|error| {
                        extraction_error(
                            "unsafe_destination",
                            format!("Cannot securely create destination directory: {error}"),
                        )
                    })?;
                continue;
            }

            let parent = root
                .parent_for(destination, &plan.destination, &mut created, &mut dir_cache)
                .map_err(|error| {
                    extraction_error(
                        "unsafe_destination",
                        format!("Cannot securely traverse destination: {error}"),
                    )
                })?;
            let file_name = plan.destination.file_name().ok_or_else(|| {
                extraction_error(
                    "unsafe_destination",
                    "Archive entry has no destination name.",
                )
            })?;
            let mut final_name = file_name.encode_wide().collect::<Vec<_>>();
            let mut overwrite = false;

            // Handle-relative existence probe (no path remove_file / no sole
            // reliance on path symlink_metadata once parent handle is open).
            {
                match parent.try_probe_file(&final_name).map_err(|error| {
                    extraction_error(
                        "write_failed",
                        format!("Cannot inspect destination entry: {error}"),
                    )
                })? {
                    LeafProbe::NotFound => {
                        // Free path — including races that cleared a prior occupant.
                    }
                    LeafProbe::Reparse => {
                        return Err(extraction_error(
                            "unsafe_destination",
                            "Destination entry is a symbolic link or reparse point.",
                        ));
                    }
                    LeafProbe::Directory => {
                        return Err(extraction_error(
                            "conflict",
                            "Destination entry already exists as a directory.",
                        ));
                    }
                    LeafProbe::File => {
                        match resolve_existing_file_dest(
                            operation_id,
                            plan,
                            &final_name,
                            conflict_resolver,
                            &mut skipped_files,
                        )? {
                            Some(name) => {
                                overwrite = name == final_name;
                                final_name = name;
                            }
                            None => continue,
                        }
                    }
                }

                let temp_name = format!(".archi-part-{}-{}", std::process::id(), plan.index)
                    .encode_utf16()
                    .collect::<Vec<_>>();
                let created_index = created.len();
                let output = parent
                    .create_file(&temp_name, &mut created)
                    .map_err(|error| {
                        extraction_error(
                            "write_failed",
                            format!("Cannot securely create temporary file: {error}"),
                        )
                    })?;
                // zip 2.x rejects by_index_decrypt on non-encrypted entries even
                // with a correct password (InvalidPassword) — decrypt only encrypted ones.
                let entry_encrypted = archive
                    .by_index_raw(plan.index)
                    .map(|e| e.encrypted())
                    .unwrap_or(false);
                let mut entry = match password.filter(|_| entry_encrypted) {
                    Some(pw) => archive
                        .by_index_decrypt(plan.index, pw.as_bytes())
                        .map_err(|error| {
                            let lower = error.to_string().to_ascii_lowercase();
                            if matches!(error, zip::result::ZipError::InvalidPassword)
                                || lower.contains("password")
                                || lower.contains("decrypt")
                            {
                                extraction_error(
                                    "password_required",
                                    "Invalid password provided. Please try again.",
                                )
                            } else {
                                extraction_error(
                                    "invalid_archive",
                                    format!("Cannot read ZIP entry: {error}"),
                                )
                            }
                        })?,
                    None => archive.by_index(plan.index).map_err(|error| {
                        extraction_error(
                            "invalid_archive",
                            format!("Cannot read ZIP entry: {error}"),
                        )
                    })?,
                };
                let entry_size = entry.size();
                if entry_size > 0 {
                    let _ = output.as_ref().set_len(entry_size);
                }
                let mut buffer = [0; BUFFER_SIZE];
                {
                    let mut writer = output.as_ref();
                    loop {
                        if cancelled.load(Ordering::Relaxed) {
                            return Err(cancelled_error());
                        }
                        let read = entry.read(&mut buffer).map_err(|error| {
                            let lower = error.to_string().to_ascii_lowercase();
                            if lower.contains("password")
                                || lower.contains("decrypt")
                                || lower.contains("authentication")
                            {
                                extraction_error(
                                    "password_required",
                                    "Invalid password provided. Please try again.",
                                )
                            } else {
                                extraction_error(
                                    "invalid_archive",
                                    format!("Cannot read ZIP data: {error}"),
                                )
                            }
                        })?;
                        if read == 0 {
                            break;
                        }
                        writer.write_all(&buffer[..read]).map_err(|error| {
                            extraction_error(
                                "write_failed",
                                format!("Cannot write extracted file: {error}"),
                            )
                        })?;
                    }
                    writer.flush().map_err(|error| {
                        extraction_error(
                            "write_failed",
                            format!("Cannot flush extracted file: {error}"),
                        )
                    })?;
                }
                if cancelled.load(Ordering::Relaxed) {
                    return Err(cancelled_error());
                }
                output.sync_all().map_err(|e| {
                    extraction_error("write_failed", format!("Cannot sync extracted file: {e}"))
                })?;
                drop(output);
                let created_file = created.get(created_index).ok_or_else(|| {
                    extraction_error("write_failed", "Temporary file tracking was unavailable.")
                })?;
                parent
                    .publish_file(created_file, &final_name, overwrite)
                    .map_err(|error| {
                        extraction_error(
                            "write_failed",
                            format!("Cannot securely finalize extracted file: {error}"),
                        )
                    })?;
                // A later entry failure must not remove an already completed replacement.
                if overwrite {
                    created.remove(created_index);
                }
            }
            extracted_files += 1;
        }
        emit(OperationProgress {
            operation_id: operation_id.into(),
            extracted_files,
            total_files,
            current_file: "Completed".into(),
            percentage: 100.0,
            phase: None,
        });
        Ok(OperationSummary {
            operation_id: operation_id.into(),
            extracted_files,
            total_files,
            skipped_files,
            destination: destination.to_string_lossy().into_owned(),
        })
    })();

    // Release cached directory handles before cleanup so DELETE disposition can complete.
    drop(dir_cache);

    match result {
        Ok(summary) => Ok(summary),
        Err(mut error) => {
            let cleanup_failures = cleanup_windows_created(&mut created);
            if !cleanup_failures.is_empty() {
                error.message.push_str(&format!(
                    " Cleanup failed for: {}.",
                    cleanup_failures.join(", ")
                ));
            }
            Err(error)
        }
    }
}

/// Format-agnostic extract entry point (zip / tar / tar.gz / gzip).
pub fn extract_any(
    archive_path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    password: Option<String>,
    conflict_resolver: &dyn ConflictResolver,
    emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => extract_archive(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            password,
            conflict_resolver,
            emit,
        ),
        ArchiveFormat::Tar => extract_tar(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            emit,
        ),
        ArchiveFormat::TarGz => extract_tar_gz(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            emit,
        ),
        ArchiveFormat::Gzip => extract_gzip(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            emit,
        ),
        ArchiveFormat::TarBz2 => extract_tar_bz2(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            emit,
        ),
        ArchiveFormat::Bzip2 => extract_bzip2(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            emit,
        ),
        ArchiveFormat::TarXz => extract_tar_xz(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            emit,
        ),
        ArchiveFormat::Xz => extract_xz(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            emit,
        ),
        ArchiveFormat::SevenZ => extract_sevenz(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            password,
            emit,
        ),
        ArchiveFormat::Rar => extract_rar(
            archive_path,
            destination,
            operation_id,
            cancelled,
            selected_paths,
            conflict_resolver,
            password,
            emit,
        ),
    }
}

pub fn extract_archive(
    zip_path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    password: Option<String>,
    conflict_resolver: &dyn ConflictResolver,
    mut emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(extraction_error(
            "invalid_operation",
            "Operation ID is empty.",
        ));
    }
    if !zip_path.is_file() {
        return Err(extraction_error(
            "not_found",
            "Source ZIP file does not exist.",
        ));
    }
    if !destination.is_dir() {
        return Err(extraction_error(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }

    let destination = destination.canonicalize().map_err(|error| {
        extraction_error(
            "invalid_destination",
            format!("Cannot resolve destination: {error}"),
        )
    })?;
    let file = fs::File::open(zip_path).map_err(|error| {
        extraction_error("invalid_archive", format!("Cannot open archive: {error}"))
    })?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        extraction_error(
            "invalid_archive",
            format!("Cannot read ZIP structure: {error}"),
        )
    })?;

    let selection_index = match selected_paths {
        Some(sel) if sel.is_empty() => {
            return Err(extraction_error(
                "empty_selection",
                "No archive entries were selected for extraction.",
            ));
        }
        Some(sel) => Some(SelectionIndex::from_selected(sel)?),
        None => None,
    };

    // Single central-directory walk: collect names for selection validation + build extract plan.
    // Full extract skips archive_names (no O(n) selection list when nothing is selected).
    let mut archive_names = if selection_index.is_some() {
        Vec::with_capacity(archive.len())
    } else {
        Vec::new()
    };
    let mut plans = Vec::with_capacity(archive.len());
    let mut planned_paths = HashSet::with_capacity(archive.len());
    let mut has_encrypted = false;
    for index in 0..archive.len() {
        // Bounded cancel ticks so large archives can abort during plan, not only extract.
        if index % PLAN_CANCEL_CHECK_INTERVAL == 0 && cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        // Raw reader for planning: metadata only; encrypted entries are opened
        // with the password later in the data pass.
        let entry = archive.by_index_raw(index).map_err(|error| {
            extraction_error("invalid_archive", format!("Cannot read ZIP entry: {error}"))
        })?;
        let name = entry.name().to_string();
        if entry.encrypted() {
            has_encrypted = true;
        }
        let name_norm = normalize_entry_name(&name);
        if let Some(ref idx) = selection_index {
            archive_names.push(name.clone());
            if !idx.includes_normalized(&name_norm) {
                continue;
            }
        }
        if is_zip_symlink(entry.unix_mode()) {
            return Err(extraction_error(
                "invalid_entry",
                "ZIP symbolic links are not supported.",
            ));
        }
        let entry_destination = safe_destination_path_under_canonical(&destination, &name)
            .map_err(|message| {
                extraction_error(
                    destination_path_error_code(&message),
                    format!("{name}: {message}"),
                )
            })?;
        let is_directory = entry.is_dir() || name.ends_with('/') || name.ends_with('\\');

        if !planned_paths.insert(entry_destination.clone()) {
            return Err(extraction_error(
                "conflict",
                "Archive contains duplicate destination paths.",
            ));
        }
        match fs::symlink_metadata(&entry_destination) {
            Ok(metadata) => plan_existing_destination(is_directory, &metadata)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(extraction_error(
                    "write_failed",
                    format!("Cannot inspect destination entry: {error}"),
                ));
            }
        }
        plans.push(PlannedEntry {
            index,
            name,
            destination: entry_destination,
            is_directory,
        });
    }
    if let Some(sel) = selected_paths {
        validate_selection(sel, &archive_names)?;
        if plans.is_empty() {
            return Err(extraction_error(
                "empty_selection",
                "No archive entries were selected for extraction.",
            ));
        }
    }
    let planned_files: HashSet<&Path> = plans
        .iter()
        .filter(|plan| !plan.is_directory)
        .map(|plan| plan.destination.as_path())
        .collect();
    for plan in &plans {
        let mut parent = plan.destination.parent();
        while let Some(path) = parent {
            if path == destination {
                break;
            }
            if planned_files.contains(path) {
                return Err(extraction_error(
                    "conflict",
                    "Archive entries conflict between a file and a directory.",
                ));
            }
            parent = path.parent();
        }
    }

    if has_encrypted && password.is_none() {
        return Err(extraction_error(
            "password_required",
            "Archive is encrypted. Password required.",
        ));
    }

    #[cfg(windows)]
    return extract_windows(
        &mut archive,
        &destination,
        operation_id,
        cancelled,
        password.as_deref(),
        conflict_resolver,
        &mut emit,
        &plans,
    );

    #[cfg(not(windows))]
    {
        let _ = conflict_resolver;
        let _ = emit;
        return Err(extraction_error(
            "unsupported_platform",
            "Safe extraction is currently available on Windows only.",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transaction_test_dir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("archi-transaction-{label}-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn selection_index_exact_match() {
        let idx = SelectionIndex::from_selected(&["readme.txt".into(), "a/b.txt".into()])
            .expect("valid selection");
        assert!(idx.includes_normalized("readme.txt"));
        assert!(idx.includes_entry(r"a\b.txt"));
        assert!(!idx.includes_normalized("other.txt"));
        assert!(!idx.includes_normalized("readme"));
    }

    #[test]
    fn selection_index_folder_prefix_matches_children() {
        let idx = SelectionIndex::from_selected(&["folder".into()]).expect("valid selection");
        assert!(idx.includes_normalized("folder"));
        assert!(idx.includes_normalized("folder/child.txt"));
        assert!(idx.includes_normalized("folder/sub/deep.txt"));
        assert!(idx.includes_entry(r"folder\child.txt"));
    }

    #[test]
    fn selection_index_rejects_empty_path() {
        let err = SelectionIndex::from_selected(&["".into()]).unwrap_err();
        assert_eq!(err.code, "invalid_selection");

        let err = SelectionIndex::from_selected(&["///".into()]).unwrap_err();
        assert_eq!(err.code, "invalid_selection");
    }

    #[test]
    fn selection_index_no_false_positive_folder_extra() {
        let idx = SelectionIndex::from_selected(&["folder".into()]).expect("valid selection");
        assert!(!idx.includes_normalized("folder_extra"));
        assert!(!idx.includes_normalized("folder_extra/x.txt"));
        assert!(!idx.includes_normalized("fold"));
        assert!(!idx.includes_normalized("folder2/x.txt"));
    }

    #[test]
    fn transaction_snapshot_restores_changed_and_created_entries() {
        let root = transaction_test_dir("rollback");
        let destination = root.join("destination");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("existing.txt"), b"old").unwrap();
        fs::create_dir(destination.join("nested")).unwrap();
        fs::write(destination.join("nested/keep.txt"), b"keep").unwrap();
        let backup = transaction_backup_path(&destination);
        copy_transaction_tree(&destination, &backup).unwrap();

        fs::write(destination.join("existing.txt"), b"new").unwrap();
        fs::write(destination.join("created.txt"), b"remove").unwrap();
        fs::remove_dir_all(destination.join("nested")).unwrap();
        restore_transaction_tree(&backup, &destination).unwrap();

        assert_eq!(fs::read(destination.join("existing.txt")).unwrap(), b"old");
        assert_eq!(
            fs::read(destination.join("nested/keep.txt")).unwrap(),
            b"keep"
        );
        assert!(!destination.join("created.txt").exists());
        fs::remove_dir_all(backup).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
