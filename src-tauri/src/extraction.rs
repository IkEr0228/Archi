use crate::bzip2_format::extract_bzip2;
#[cfg(windows)]
use crate::conflict::unique_renamed_path;
use crate::format_detect::{detect_format, ArchiveFormat};
use crate::gzip_format::extract_gzip;
use crate::models::{CommandError, ConflictDecision, OperationProgress, OperationSummary};
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

/// Pre-normalized selection for O(1)/O(k) membership without per-call allocs.
#[derive(Debug, Clone)]
pub struct SelectionIndex {
    exact: HashSet<String>,
    /// Prefixes stored as `folder/` for fast `starts_with`.
    prefixes: Vec<String>,
}

impl SelectionIndex {
    pub fn from_selected(selected: &[String]) -> Result<Self, CommandError> {
        let mut exact = HashSet::with_capacity(selected.len());
        let mut prefixes = Vec::with_capacity(selected.len());
        for raw in selected {
            let sel = normalize_entry_name(raw);
            if sel.is_empty() {
                return Err(extraction_error(
                    "invalid_selection",
                    "Selection contains an empty path.",
                ));
            }
            prefixes.push(format!("{sel}/"));
            exact.insert(sel);
        }
        Ok(Self { exact, prefixes })
    }

    #[inline]
    pub fn includes_normalized(&self, entry: &str) -> bool {
        if self.exact.contains(entry) {
            return true;
        }
        self.prefixes.iter().any(|p| entry.starts_with(p.as_str()))
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
    // Ensure every selected path is covered by at least one archive entry.
    for sel in &index.exact {
        let ok = archive_names.iter().any(|n| {
            let entry = normalize_entry_name(n);
            entry_matches_sel(&entry, sel)
        });
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
/// Overwrite deletes the leaf via the parent directory handle (disposition),
/// never via path-based `remove_file`.
#[cfg(windows)]
fn resolve_existing_file_dest(
    operation_id: &str,
    plan: &PlannedEntry,
    parent: &Directory,
    leaf_name: &[u16],
    conflict_resolver: &dyn ConflictResolver,
    skipped_files: &mut u64,
) -> Result<Option<Vec<u16>>, CommandError> {
    let decision =
        conflict_resolver.resolve_file_exists(operation_id, &plan.name, &plan.destination)?;

    match decision {
        ConflictDecision::Overwrite => {
            parent.delete_file_by_name(leaf_name).map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidInput {
                    extraction_error(
                        "unsafe_destination",
                        "Destination entry is a symbolic link or reparse point.",
                    )
                } else {
                    extraction_error(
                        "write_failed",
                        format!("Cannot remove existing destination file: {error}"),
                    )
                }
            })?;
            Ok(Some(leaf_name.to_vec()))
        }
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
    let mut dir_cache = std::collections::HashMap::new();
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
                            &parent,
                            &final_name,
                            conflict_resolver,
                            &mut skipped_files,
                        )? {
                            Some(name) => final_name = name,
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
                let mut entry = archive.by_index(plan.index).map_err(|error| {
                    extraction_error("invalid_archive", format!("Cannot read ZIP entry: {error}"))
                })?;
                let mut buffer = [0; BUFFER_SIZE];
                {
                    let mut writer = output.as_ref();
                    loop {
                        if cancelled.load(Ordering::Relaxed) {
                            return Err(cancelled_error());
                        }
                        let read = entry.read(&mut buffer).map_err(|error| {
                            extraction_error(
                                "invalid_archive",
                                format!("Cannot read ZIP data: {error}"),
                            )
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
                drop(output);
                let created_file = created.get(created_index).ok_or_else(|| {
                    extraction_error("write_failed", "Temporary file tracking was unavailable.")
                })?;
                parent
                    .rename_new_file(created_file, &final_name)
                    .map_err(|error| {
                        extraction_error(
                            "write_failed",
                            format!("Cannot securely finalize extracted file: {error}"),
                        )
                    })?;
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
    for index in 0..archive.len() {
        // Bounded cancel ticks so large archives can abort during plan, not only extract.
        if index % PLAN_CANCEL_CHECK_INTERVAL == 0 && cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        let entry = archive.by_index(index).map_err(|error| {
            extraction_error("invalid_archive", format!("Cannot read ZIP entry: {error}"))
        })?;
        let name = entry.name().to_string();
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

    #[cfg(windows)]
    return extract_windows(
        &mut archive,
        &destination,
        operation_id,
        cancelled,
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
}
