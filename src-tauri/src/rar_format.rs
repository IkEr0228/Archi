//! RAR open/list and extract implementation using `unrar` (read/extract only).

use crate::conflict::unique_renamed_path;
use crate::extraction::{validate_selection, ConflictResolver, SelectionIndex};
use crate::models::{
    ArchiveCapabilities, ArchiveEntry, ArchiveInfo, ArchiveStats, CommandError, ConflictDecision,
    OperationProgress, OperationSummary,
};
use crate::security::{
    assess_archive, safe_destination_path_under_canonical, validate_entry_path, ArchiveRiskInput,
};
use ahash::AHashMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use unrar::error::{Code, UnrarError};
use unrar::Archive;

fn rar_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

pub(crate) fn map_unrar_error(err: UnrarError) -> CommandError {
    match err.code {
        Code::MissingPassword => {
            rar_error("password_required", "Archive requires a password.")
        }
        Code::BadPassword => {
            rar_error("wrong_password", "Invalid password provided.")
        }
        Code::BadData => {
            rar_error("corrupted_archive", "Archive data or CRC is corrupted.")
        }
        Code::BadArchive => {
            rar_error("invalid_archive", "File is not a valid RAR archive.")
        }
        Code::NoMemory => {
            rar_error("out_of_memory", "Not enough memory to process RAR archive.")
        }
        Code::UnknownFormat => {
            rar_error("unsupported_format", "Unknown or unsupported RAR format/encryption.")
        }
        _ => rar_error("read_failed", err.to_string()),
    }
}

fn rar_capabilities() -> ArchiveCapabilities {
    ArchiveCapabilities {
        open: true,
        list: true,
        extract: true,
        create: false,
        edit: false,
        encrypt: false,
        test: false,
    }
}

fn normalize_member_name(raw: &str) -> Result<String, CommandError> {
    let mut normalized = raw.replace('\\', "/");
    while normalized.starts_with("./") {
        normalized = normalized[2..].to_string();
    }
    normalized = normalized.trim_matches('/').to_string();
    if normalized.is_empty() {
        return Err(rar_error("invalid_entry", "Archive entry path is empty."));
    }
    validate_entry_path(&normalized).map_err(|message| CommandError {
        code: "invalid_entry".into(),
        message,
        path: Some(normalized.clone()),
    })?;
    Ok(normalized)
}

fn format_dos_datetime(file_time: u32) -> Option<String> {
    if file_time == 0 {
        return None;
    }
    let sec = (file_time & 0x1f) * 2;
    let min = (file_time >> 5) & 0x3f;
    let hour = (file_time >> 11) & 0x1f;
    let day = (file_time >> 16) & 0x1f;
    let month = (file_time >> 21) & 0x0f;
    let year = 1980 + ((file_time >> 25) & 0x7f);

    if (1..=12).contains(&month) && (1..=31).contains(&day) && hour <= 23 && min <= 59 && sec <= 59 {
        Some(format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z"))
    } else {
        None
    }
}

/// Open a RAR archive for listing metadata.
pub fn open_rar(path: &Path, password: Option<&str>) -> Result<ArchiveInfo, CommandError> {
    if !path.is_file() {
        return Err(rar_error("not_found", "File not found or is not a file."));
    }

    let archive = match password {
        Some(pw) if !pw.is_empty() => Archive::with_password(path, pw),
        _ => Archive::new(path),
    };

    let open_archive = archive.open_for_listing().map_err(map_unrar_error)?;

    let mut entries: Vec<ArchiveEntry> = Vec::new();
    let mut entry_indices: AHashMap<String, usize> = AHashMap::new();
    let mut total_uncompressed: u64 = 0;
    let mut largest_entry: u64 = 0;
    let mut deepest_path = 0_usize;
    let mut physical = 0_usize;

    for header_result in open_archive {
        let header = header_result.map_err(map_unrar_error)?;
        let raw_name = header.filename.to_string_lossy().to_string();
        let normalized = normalize_member_name(&raw_name)?;

        physical = physical.saturating_add(1);
        let is_dir = header.is_directory();
        if !is_dir {
            total_uncompressed = total_uncompressed.saturating_add(header.unpacked_size);
            largest_entry = largest_entry.max(header.unpacked_size);
        }
        deepest_path = deepest_path.max(normalized.split('/').count());

        let parts: Vec<&str> = normalized.split('/').collect();
        let mut current_prefix = String::new();
        for (j, part) in parts.iter().enumerate() {
            let parent = if current_prefix.is_empty() {
                "/".to_string()
            } else {
                current_prefix.clone()
            };
            if !current_prefix.is_empty() {
                current_prefix.push('/');
            }
            current_prefix.push_str(part);
            let component_is_dir = j < parts.len() - 1 || is_dir;

            if let Some(&entry_index) = entry_indices.get(&current_prefix) {
                if j == parts.len() - 1 && is_dir {
                    let entry = &mut entries[entry_index];
                    entry.is_directory = true;
                    entry.uncompressed_size = 0;
                    entry.compressed_size = None;
                    entry.method = None;
                }
            } else {
                let uncompressed_size = if component_is_dir { 0 } else { header.unpacked_size };
                let modified_at = if j == parts.len() - 1 {
                    format_dos_datetime(header.file_time)
                } else {
                    None
                };

                entries.push(ArchiveEntry {
                    path: current_prefix.clone(),
                    name: (*part).to_string(),
                    parent_path: parent,
                    is_directory: component_is_dir,
                    uncompressed_size,
                    compressed_size: None,
                    modified_at,
                    method: (!component_is_dir).then(|| "RAR".into()),
                });
                entry_indices.insert(current_prefix.clone(), entries.len() - 1);
            }
        }
    }

    let mut file_count = 0_u64;
    let mut folder_count = 0_u64;
    let mut methods = BTreeSet::new();
    for entry in &entries {
        if entry.is_directory {
            folder_count += 1;
        } else {
            file_count += 1;
            if let Some(ref m) = entry.method {
                methods.insert(m.clone());
            }
        }
    }

    let on_disk_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let total_compressed = on_disk_size;

    let entry_count = physical.max(entries.len());
    let risk_input = ArchiveRiskInput {
        entry_count,
        total_uncompressed,
        total_compressed,
        largest_entry,
        deepest_path,
    };
    let warnings = assess_archive(risk_input);

    Ok(ArchiveInfo {
        archive_path: path.to_string_lossy().into_owned(),
        format: "rar".into(),
        entries,
        capabilities: rar_capabilities(),
        warnings,
        stats: ArchiveStats {
            file_count,
            folder_count,
            total_uncompressed,
            total_compressed,
            methods: methods.into_iter().collect(),
        },
    })
}

/// Extract RAR archive with path validation, selection filtering, conflict resolution, and cancellation.
pub fn extract_rar(
    archive_path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    password: Option<String>,
    mut emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(rar_error("invalid_operation", "Operation ID is empty."));
    }
    if !archive_path.is_file() {
        return Err(rar_error("not_found", "Source archive does not exist."));
    }
    if !destination.is_dir() {
        return Err(rar_error(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }
    let destination = destination.canonicalize().map_err(|error| {
        rar_error(
            "invalid_destination",
            format!("Cannot resolve destination: {error}"),
        )
    })?;

    // Pre-scan listing to validate selection and compute total files
    let (names, selection_index) = {
        let arch = match password.as_deref() {
            Some(pw) if !pw.is_empty() => Archive::with_password(archive_path, pw),
            _ => Archive::new(archive_path),
        };
        let lister = arch.open_for_listing().map_err(map_unrar_error)?;
        let mut list = Vec::new();
        for h in lister {
            let hdr = h.map_err(map_unrar_error)?;
            let raw_name = hdr.filename.to_string_lossy().to_string();
            if let Ok(n) = normalize_member_name(&raw_name) {
                list.push(n);
            }
        }
        let sel_idx = match selected_paths {
            Some(sel) if sel.is_empty() => {
                return Err(rar_error(
                    "empty_selection",
                    "No archive entries were selected for extraction.",
                ));
            }
            Some(sel) => {
                validate_selection(sel, &list)?;
                Some(SelectionIndex::from_selected(sel)?)
            }
            None => None,
        };
        (list, sel_idx)
    };

    let total_files = match &selection_index {
        Some(idx) => names.iter().filter(|n| idx.includes_normalized(n)).count() as u64,
        None => names.len() as u64,
    }
    .max(1);

    let archive = match password.as_deref() {
        Some(pw) if !pw.is_empty() => Archive::with_password(archive_path, pw),
        _ => Archive::new(archive_path),
    };

    let mut open_archive = archive.open_for_processing().map_err(map_unrar_error)?;
    let mut extracted_files = 0_u64;
    let mut skipped_files = 0_u64;

    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err(rar_error("cancelled", "Extraction was cancelled."));
        }

        let cursor = match open_archive.read_header() {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => return Err(map_unrar_error(e)),
        };

        let raw_name = cursor.entry().filename.to_string_lossy().to_string();
        let normalized = match normalize_member_name(&raw_name) {
            Ok(n) => n,
            Err(e) => return Err(e),
        };

        let include = match &selection_index {
            None => true,
            Some(idx) => idx.includes_normalized(&normalized),
        };

        if !include {
            open_archive = cursor.skip().map_err(map_unrar_error)?;
            continue;
        }

        let is_dir = cursor.entry().is_directory();
        let target_path = match safe_destination_path_under_canonical(&destination, &normalized) {
            Ok(p) => p,
            Err(msg) => return Err(rar_error("unsafe_destination", msg)),
        };

        if is_dir {
            fs::create_dir_all(&target_path).map_err(|e| {
                rar_error("write_failed", format!("Cannot create directory: {e}"))
            })?;
            open_archive = cursor.skip().map_err(map_unrar_error)?;
            continue;
        }

        // Ensure parent directory exists
        if let Some(parent) = target_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    rar_error("write_failed", format!("Cannot create parent directory: {e}"))
                })?;
            }
        }

        let mut write_target = target_path.clone();
        if target_path.exists() {
            let decision = conflict_resolver.resolve_file_exists(operation_id, &normalized, &target_path)?;
            match decision {
                ConflictDecision::Overwrite => {
                    let _ = fs::remove_file(&target_path);
                }
                ConflictDecision::Skip => {
                    skipped_files += 1;
                    open_archive = cursor.skip().map_err(map_unrar_error)?;
                    continue;
                }
                ConflictDecision::Rename => {
                    let parent = target_path.parent().ok_or_else(|| {
                        rar_error("unsafe_destination", "Target has no parent.")
                    })?;
                    let file_name = target_path.file_name().ok_or_else(|| {
                        rar_error("unsafe_destination", "Target has no file name.")
                    })?.to_string_lossy();
                    write_target = unique_renamed_path(parent, &file_name)?;
                }
                ConflictDecision::Cancel => {
                    return Err(rar_error("cancelled", "Extraction cancelled by user."));
                }
            }
        }

        open_archive = cursor.extract_to(&write_target).map_err(map_unrar_error)?;
        extracted_files += 1;

        emit(OperationProgress {
            operation_id: operation_id.to_string(),
            extracted_files,
            total_files,
            current_file: normalized,
            percentage: (extracted_files as f32 / total_files as f32).min(1.0),
            phase: Some("extract".into()),
        });
    }

    Ok(OperationSummary {
        operation_id: operation_id.to_string(),
        extracted_files,
        total_files,
        skipped_files,
        destination: destination.to_string_lossy().into_owned(),
    })
}
