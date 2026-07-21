//! TAR and TAR.GZ open/list/extract.

#[cfg(windows)]
use crate::conflict::unique_renamed_path;
use crate::extraction::{
    normalize_entry_name, validate_selection, ConflictResolver, SelectionIndex,
};
use crate::models::{
    ArchiveCapabilities, ArchiveEntry, ArchiveInfo, ArchiveStats, CommandError, ConflictDecision,
    OperationProgress, OperationSummary,
};
use crate::security::{
    assess_archive, destination_path_error_code, is_link_or_reparse_point,
    safe_destination_path_under_canonical, validate_entry_path, ArchiveRiskInput,
};
#[cfg(windows)]
use crate::windows_fs::{cleanup_created as cleanup_windows_created, Directory};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tar::{Archive, EntryType};
use xz2::read::XzDecoder;

use crate::io_perf::{IO_BUFFER_SIZE as BUFFER_SIZE, PROGRESS_INTERVAL};

fn tar_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn read_only_capabilities() -> ArchiveCapabilities {
    ArchiveCapabilities {
        open: true,
        list: true,
        extract: true,
        create: false,
        // Multi-entry TAR: integrity test yes; stream-rebuild edit (see tar_edit).
        edit: true,
        encrypt: false,
        test: true,
    }
}

struct ListedMember {
    /// Logical archive path (no trailing slash).
    path: String,
    is_directory: bool,
    size: u64,
    /// True if tar member is symlink/hardlink/device — not extractable.
    is_link_or_special: bool,
    modified_at: Option<String>,
}

fn entry_modified(entry: &tar::Entry<impl Read>) -> Option<String> {
    entry.header().mtime().ok().map(|mtime| {
        // Store as unix seconds string; UI already shows raw modified strings.
        format!("{mtime}")
    })
}

fn collect_tar_members<R: Read>(
    archive: &mut Archive<R>,
) -> Result<Vec<ListedMember>, CommandError> {
    let mut members = Vec::new();
    let entries = archive.entries().map_err(|error| {
        tar_error(
            "invalid_archive",
            format!("Cannot read tar entries: {error}"),
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            tar_error("invalid_archive", format!("Cannot read tar entry: {error}"))
        })?;
        let path = entry.path().map_err(|error| {
            tar_error("invalid_entry", format!("Invalid tar entry path: {error}"))
        })?;
        let raw = path.to_string_lossy().replace('\\', "/");
        validate_entry_path(&raw).map_err(|message| CommandError {
            code: "invalid_entry".into(),
            message,
            path: Some(raw.clone()),
        })?;
        let normalized = normalize_entry_name(&raw);
        if normalized.is_empty() {
            return Err(CommandError {
                code: "invalid_entry".into(),
                message: "Archive entry path is empty or malformed.".into(),
                path: Some(raw),
            });
        }

        let header = entry.header();
        let entry_type = header.entry_type();
        let is_dir = entry_type.is_dir() || raw.ends_with('/');
        let is_link_or_special = matches!(
            entry_type,
            EntryType::Symlink
                | EntryType::Link
                | EntryType::Char
                | EntryType::Block
                | EntryType::Fifo
                | EntryType::Continuous
        ) || entry_type.is_symlink()
            || entry_type.is_hard_link();

        let size = if is_dir || is_link_or_special {
            0
        } else {
            header.size().unwrap_or(0)
        };

        members.push(ListedMember {
            path: normalized,
            is_directory: is_dir,
            size,
            is_link_or_special,
            modified_at: entry_modified(&entry),
        });
    }
    Ok(members)
}

fn build_archive_info(
    archive_path: &Path,
    format: &str,
    members: &[ListedMember],
    total_compressed: u64,
) -> ArchiveInfo {
    // Virtual parents roughly double entry count in deep trees; reserve modestly.
    let reserve = members.len().saturating_mul(2).max(16);
    let mut entries: Vec<ArchiveEntry> = Vec::with_capacity(reserve);
    let mut entry_indices: HashMap<String, usize> = HashMap::with_capacity(reserve);
    let mut total_uncompressed: u64 = 0;
    let mut largest_entry: u64 = 0;
    let mut deepest_path = 0_usize;
    let mut physical_entry_count = 0_usize;

    for member in members {
        if member.is_link_or_special {
            // Listed only as non-extractable awareness is optional; skip from tree for v1.
            continue;
        }
        physical_entry_count = physical_entry_count.saturating_add(1);
        total_uncompressed = total_uncompressed.saturating_add(member.size);
        largest_entry = largest_entry.max(member.size);
        deepest_path = deepest_path.max(member.path.split('/').count());

        let parts: Vec<&str> = member.path.split('/').collect();
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
            let component_is_dir = j < parts.len() - 1 || member.is_directory;

            if let Some(&entry_index) = entry_indices.get(&current_prefix) {
                if j == parts.len() - 1 && member.is_directory {
                    let entry = &mut entries[entry_index];
                    entry.is_directory = true;
                    entry.uncompressed_size = 0;
                    entry.compressed_size = None;
                    entry.modified_at = member.modified_at.clone();
                    entry.method = None;
                }
            } else {
                let uncompressed_size = if component_is_dir { 0 } else { member.size };
                // Per-entry packed size for stream formats is filled after we know on-disk size.
                let compressed_size = None;
                let method = if component_is_dir {
                    None
                } else if format == "tar.gz" {
                    Some("Gzip".into())
                } else if format == "tar.bz2" {
                    Some("Bzip2".into())
                } else if format == "tar.xz" {
                    Some("XZ".into())
                } else {
                    Some("Stored".into())
                };
                entries.push(ArchiveEntry {
                    path: current_prefix.clone(),
                    name: (*part).to_string(),
                    parent_path: parent,
                    is_directory: component_is_dir,
                    uncompressed_size,
                    compressed_size,
                    modified_at: (j == parts.len() - 1)
                        .then(|| member.modified_at.clone())
                        .flatten(),
                    method,
                });
                entry_indices.insert(current_prefix.clone(), entries.len() - 1);
            }
        }
    }

    let on_disk = fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0);
    let total_compressed = if total_compressed > 0 {
        total_compressed
    } else {
        on_disk
    };

    // Stream formats (and plain tar) do not store per-member packed sizes.
    // Allocate on-disk archive size across files by uncompressed share so the
    // table ratio/compressed columns reflect overall packing.
    allocate_stream_compressed_sizes(&mut entries, total_compressed, total_uncompressed);

    let mut file_count = 0_u64;
    let mut folder_count = 0_u64;
    let mut methods = BTreeSet::new();
    for entry in &entries {
        if entry.is_directory {
            folder_count += 1;
        } else {
            file_count += 1;
            if let Some(method) = &entry.method {
                methods.insert(method.clone());
            }
        }
    }

    let entry_count = physical_entry_count.max(entries.len());
    let warnings = assess_archive(ArchiveRiskInput {
        entry_count,
        total_uncompressed,
        total_compressed,
        largest_entry,
        deepest_path,
    });

    ArchiveInfo {
        archive_path: archive_path.to_string_lossy().into_owned(),
        format: format.into(),
        entries,
        capabilities: read_only_capabilities(),
        warnings,
        stats: ArchiveStats {
            file_count,
            folder_count,
            total_uncompressed,
            total_compressed,
            methods: methods.into_iter().collect(),
        },
    }
}

/// Spread `packed_total` (usually on-disk archive length) across file entries by
/// uncompressed size share. Last file gets the remainder so the sum matches.
fn allocate_stream_compressed_sizes(
    entries: &mut [ArchiveEntry],
    packed_total: u64,
    total_uncompressed: u64,
) {
    let file_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.is_directory)
        .map(|(index, _)| index)
        .collect();
    if file_indices.is_empty() {
        return;
    }

    if total_uncompressed == 0 {
        let each = packed_total / file_indices.len() as u64;
        let mut assigned = 0_u64;
        for (k, &index) in file_indices.iter().enumerate() {
            let share = if k + 1 == file_indices.len() {
                packed_total.saturating_sub(assigned)
            } else {
                assigned = assigned.saturating_add(each);
                each
            };
            entries[index].compressed_size = Some(share);
        }
        return;
    }

    let mut assigned = 0_u64;
    for (k, &index) in file_indices.iter().enumerate() {
        let unc = entries[index].uncompressed_size;
        let share = if k + 1 == file_indices.len() {
            packed_total.saturating_sub(assigned)
        } else {
            let s = ((packed_total as u128)
                .saturating_mul(unc as u128)
                .saturating_div(total_uncompressed as u128)) as u64;
            assigned = assigned.saturating_add(s);
            s
        };
        entries[index].compressed_size = Some(share);
    }
}

/// Open a plain `.tar` archive for listing.
pub fn open_tar(path: &Path) -> Result<ArchiveInfo, CommandError> {
    let file = File::open(path)
        .map_err(|error| tar_error("invalid_archive", format!("Failed to open tar: {error}")))?;
    let mut archive = Archive::new(file);
    let members = collect_tar_members(&mut archive)?;
    Ok(build_archive_info(path, "tar", &members, 0))
}

/// Open a `.tar.gz` / `.tgz` archive for listing.
pub fn open_tar_gz(path: &Path) -> Result<ArchiveInfo, CommandError> {
    let file = File::open(path)
        .map_err(|error| tar_error("invalid_archive", format!("Failed to open tar.gz: {error}")))?;
    let on_disk = file.metadata().map(|m| m.len()).unwrap_or(0);
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let members = collect_tar_members(&mut archive)?;
    Ok(build_archive_info(path, "tar.gz", &members, on_disk))
}

/// Open a `.tar.bz2` / `.tbz2` archive for listing.
pub fn open_tar_bz2(path: &Path) -> Result<ArchiveInfo, CommandError> {
    let file = File::open(path).map_err(|error| {
        tar_error(
            "invalid_archive",
            format!("Failed to open tar.bz2: {error}"),
        )
    })?;
    let on_disk = file.metadata().map(|m| m.len()).unwrap_or(0);
    let decoder = BzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let members = collect_tar_members(&mut archive)?;
    Ok(build_archive_info(path, "tar.bz2", &members, on_disk))
}

/// Open a `.tar.xz` / `.txz` archive for listing.
pub fn open_tar_xz(path: &Path) -> Result<ArchiveInfo, CommandError> {
    let file = File::open(path)
        .map_err(|error| tar_error("invalid_archive", format!("Failed to open tar.xz: {error}")))?;
    let on_disk = file.metadata().map(|m| m.len()).unwrap_or(0);
    let decoder = XzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let members = collect_tar_members(&mut archive)?;
    Ok(build_archive_info(path, "tar.xz", &members, on_disk))
}

#[cfg(windows)]
fn extract_one_file_windows(
    root: &Directory,
    extract_root: &Path,
    destination: &Path,
    entry_path: &str,
    mut reader: impl Read,
    operation_id: &str,
    cancelled: &AtomicBool,
    conflict_resolver: &dyn ConflictResolver,
    created: &mut Vec<crate::windows_fs::CreatedEntry>,
    dir_cache: &mut std::collections::HashMap<PathBuf, Directory>,
) -> Result<bool, CommandError> {
    use std::os::windows::ffi::OsStrExt;

    let mut write_to = destination.to_path_buf();

    loop {
        match fs::symlink_metadata(&write_to) {
            Ok(meta) => {
                if is_link_or_reparse_point(&meta) {
                    return Err(tar_error(
                        "unsafe_destination",
                        "Destination path is a reparse point.",
                    ));
                }
                if meta.is_dir() {
                    return Err(tar_error(
                        "conflict",
                        "Cannot overwrite a directory with a file.",
                    ));
                }
                let decision =
                    conflict_resolver.resolve_file_exists(operation_id, entry_path, &write_to)?;
                match decision {
                    ConflictDecision::Skip => return Ok(false),
                    ConflictDecision::Cancel => {
                        return Err(tar_error("cancelled", "Archive extraction was cancelled."))
                    }
                    ConflictDecision::Overwrite => {
                        fs::remove_file(&write_to).map_err(|error| {
                            tar_error(
                                "write_failed",
                                format!("Cannot remove existing file: {error}"),
                            )
                        })?;
                        break;
                    }
                    ConflictDecision::Rename => {
                        let parent = write_to.parent().ok_or_else(|| {
                            tar_error("invalid_destination", "Destination has no parent.")
                        })?;
                        let file_name = write_to
                            .file_name()
                            .ok_or_else(|| {
                                tar_error("invalid_destination", "Destination has no file name.")
                            })?
                            .to_string_lossy();
                        write_to = unique_renamed_path(parent, &file_name)?;
                        continue;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(tar_error(
                    "write_failed",
                    format!("Cannot inspect destination: {error}"),
                ));
            }
        }
    }

    let parent = root
        .parent_for(extract_root, &write_to, created, dir_cache)
        .map_err(|error| {
            tar_error(
                "write_failed",
                format!("Cannot create destination parents: {error}"),
            )
        })?;

    let file_name = write_to
        .file_name()
        .ok_or_else(|| tar_error("invalid_destination", "Destination has no file name."))?;
    let wide: Vec<u16> = file_name.encode_wide().collect();

    let created_index = created.len();
    let temp_name: Vec<u16> = format!(
        ".archi-part-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
    .encode_utf16()
    .collect();

    let output = parent
        .create_file(&temp_name, created)
        .map_err(|error| tar_error("write_failed", format!("Cannot create temp file: {error}")))?;

    let mut buffer = [0_u8; BUFFER_SIZE];
    {
        let mut writer = output.as_ref();
        loop {
            if cancelled.load(Ordering::Relaxed) {
                drop(output);
                let _ = cleanup_windows_created(created);
                return Err(tar_error("cancelled", "Archive extraction was cancelled."));
            }
            let n = reader.read(&mut buffer).map_err(|error| {
                tar_error(
                    "invalid_archive",
                    format!("Cannot read tar member: {error}"),
                )
            })?;
            if n == 0 {
                break;
            }
            writer.write_all(&buffer[..n]).map_err(|error| {
                tar_error(
                    "write_failed",
                    format!("Cannot write extracted file: {error}"),
                )
            })?;
        }
        writer
            .flush()
            .map_err(|error| tar_error("write_failed", format!("Cannot flush file: {error}")))?;
    }
    drop(output);

    let created_file = created
        .get(created_index)
        .ok_or_else(|| tar_error("write_failed", "Missing created temp file handle."))?;
    parent
        .rename_new_file(created_file, &wide)
        .map_err(|error| {
            tar_error(
                "write_failed",
                format!("Cannot finalize extracted file: {error}"),
            )
        })?;

    Ok(true)
}

#[cfg(not(windows))]
fn extract_one_file_windows(
    _extract_root: &Path,
    destination: &Path,
    entry_path: &str,
    mut reader: impl Read,
    operation_id: &str,
    cancelled: &AtomicBool,
    conflict_resolver: &dyn ConflictResolver,
    _created: &mut Vec<()>,
    _dir_cache: &mut std::collections::HashMap<PathBuf, ()>,
) -> Result<bool, CommandError> {
    let mut write_to = destination.to_path_buf();
    if write_to.exists() {
        let decision =
            conflict_resolver.resolve_file_exists(operation_id, entry_path, &write_to)?;
        match decision {
            ConflictDecision::Skip => return Ok(false),
            ConflictDecision::Cancel => {
                return Err(tar_error("cancelled", "Archive extraction was cancelled."))
            }
            ConflictDecision::Overwrite => {
                fs::remove_file(&write_to).map_err(|e| {
                    tar_error("write_failed", format!("Cannot remove existing file: {e}"))
                })?;
            }
            ConflictDecision::Rename => {
                return Err(tar_error(
                    "unsupported_operation",
                    "Rename conflict is Windows-only in this build.",
                ));
            }
        }
    }
    if let Some(parent) = write_to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| tar_error("write_failed", format!("Cannot create parents: {e}")))?;
    }
    let mut file = fs::File::create(&write_to)
        .map_err(|e| tar_error("write_failed", format!("Cannot create file: {e}")))?;
    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(tar_error("cancelled", "Archive extraction was cancelled."));
        }
        let n = reader
            .read(&mut buffer)
            .map_err(|e| tar_error("invalid_archive", format!("Cannot read: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])
            .map_err(|e| tar_error("write_failed", format!("Cannot write: {e}")))?;
    }
    Ok(true)
}

fn discard_tar_entry(entry: &mut impl Read) -> Result<(), CommandError> {
    let mut sink = std::io::sink();
    std::io::copy(entry, &mut sink).map_err(|error| {
        tar_error(
            "invalid_archive",
            format!("Cannot skip tar member data: {error}"),
        )
    })?;
    Ok(())
}

/// First-pass name scan for selective extract validation (no writes).
/// Must drain file bodies so the tar stream stays aligned.
fn collect_tar_stream_names<R: Read>(
    mut archive: Archive<R>,
    cancelled: &AtomicBool,
) -> Result<Vec<String>, CommandError> {
    let mut names = Vec::new();
    let entries = archive.entries().map_err(|error| {
        tar_error(
            "invalid_archive",
            format!("Cannot read tar entries: {error}"),
        )
    })?;
    for entry in entries {
        if cancelled.load(Ordering::Relaxed) {
            return Err(tar_error("cancelled", "Archive extraction was cancelled."));
        }
        let mut entry = entry.map_err(|error| {
            tar_error("invalid_archive", format!("Cannot read tar entry: {error}"))
        })?;
        let path = entry.path().map_err(|error| {
            tar_error("invalid_entry", format!("Invalid tar entry path: {error}"))
        })?;
        let raw = path.to_string_lossy().replace('\\', "/");
        // Soft-skip invalid names in list pass? No — same rules as extract: surface path errors.
        validate_entry_path(&raw).map_err(|message| CommandError {
            code: "invalid_entry".into(),
            message,
            path: Some(raw.clone()),
        })?;
        let name = normalize_entry_name(&raw);
        let header = entry.header().clone();
        let entry_type = header.entry_type();
        let is_dir = entry_type.is_dir() || raw.ends_with('/');
        names.push(name.clone());
        if is_dir {
            names.push(format!("{name}/"));
        } else {
            discard_tar_entry(&mut entry)?;
        }
    }
    Ok(names)
}

/// Validate selection against archive members **before** any destination write.
fn prevalidate_tar_selection<R: Read>(
    archive: Archive<R>,
    selected: &[String],
    cancelled: &AtomicBool,
) -> Result<(), CommandError> {
    if selected.is_empty() {
        return Err(tar_error(
            "empty_selection",
            "No archive entries were selected for extraction.",
        ));
    }
    let names = collect_tar_stream_names(archive, cancelled)?;
    validate_selection(selected, &names)
}

/// Single-pass extract: stream file bodies to disk (or discard). No full-member RAM buffer.
/// When `selected_paths` is `Some`, callers must have already run [`prevalidate_tar_selection`].
fn extract_tar_reader<R: Read>(
    mut archive: Archive<R>,
    extract_root: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    mut emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    let selection_index = match selected_paths {
        Some(sel) if sel.is_empty() => {
            return Err(tar_error(
                "empty_selection",
                "No archive entries were selected for extraction.",
            ));
        }
        Some(sel) => Some(SelectionIndex::from_selected(sel)?),
        None => None,
    };

    let mut extracted = 0_u64;
    let mut skipped = 0_u64;
    let mut planned_or_extracted = 0_u64;
    let mut last_progress = Instant::now();

    #[cfg(windows)]
    let mut created = Vec::new();
    #[cfg(windows)]
    let mut dir_cache = std::collections::HashMap::new();
    #[cfg(not(windows))]
    let mut created = Vec::new();
    #[cfg(not(windows))]
    let mut dir_cache = std::collections::HashMap::new();

    // Open destination root once per extract (mirrors ZIP extract_windows).
    #[cfg(windows)]
    let root = Directory::open_root(extract_root).map_err(|error| {
        tar_error(
            "unsafe_destination",
            format!("Cannot open destination root: {error}"),
        )
    })?;

    let result = (|| -> Result<(), CommandError> {
        let entries = archive.entries().map_err(|error| {
            tar_error(
                "invalid_archive",
                format!("Cannot read tar entries: {error}"),
            )
        })?;
        for entry in entries {
            if cancelled.load(Ordering::Relaxed) {
                return Err(tar_error("cancelled", "Archive extraction was cancelled."));
            }
            let mut entry = entry.map_err(|error| {
                tar_error("invalid_archive", format!("Cannot read tar entry: {error}"))
            })?;
            let path = entry.path().map_err(|error| {
                tar_error("invalid_entry", format!("Invalid tar entry path: {error}"))
            })?;
            let raw = path.to_string_lossy().replace('\\', "/");
            validate_entry_path(&raw).map_err(|message| CommandError {
                code: "invalid_entry".into(),
                message,
                path: Some(raw.clone()),
            })?;
            let name = normalize_entry_name(&raw);
            let header = entry.header().clone();
            let entry_type = header.entry_type();
            let is_dir = entry_type.is_dir() || raw.ends_with('/');
            let is_link = matches!(
                entry_type,
                EntryType::Symlink
                    | EntryType::Link
                    | EntryType::Char
                    | EntryType::Block
                    | EntryType::Fifo
                    | EntryType::Continuous
            ) || entry_type.is_symlink()
                || entry_type.is_hard_link();

            // `name` is already normalize_entry_name'd; SelectionIndex encodes dir prefixes.
            // Selection was pre-validated by the extract_* entry points when present.
            let include = match &selection_index {
                None => true,
                Some(idx) => idx.includes_normalized(&name),
            };

            if is_link {
                if include {
                    return Err(tar_error(
                        "unsafe_entry",
                        format!("Archive member is a link or special file: {name}"),
                    ));
                }
                discard_tar_entry(&mut entry)?;
                continue;
            }

            if !include {
                if !is_dir {
                    discard_tar_entry(&mut entry)?;
                }
                continue;
            }

            // Containment check before write (`extract_root` is already canonical).
            let dest =
                safe_destination_path_under_canonical(extract_root, &name).map_err(|message| {
                    CommandError {
                        code: destination_path_error_code(&message).into(),
                        message,
                        path: Some(name.clone()),
                    }
                })?;

            planned_or_extracted = planned_or_extracted.saturating_add(1);
            if last_progress.elapsed() >= PROGRESS_INTERVAL || planned_or_extracted == 1 {
                last_progress = Instant::now();
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files: extracted,
                    // Unknown total in single-pass; keep UI moving with processed count.
                    total_files: planned_or_extracted.max(extracted).max(1),
                    current_file: name.clone(),
                    percentage: 0.0,
                    phase: None,
                });
            }

            if is_dir {
                #[cfg(windows)]
                {
                    root.ensure_path(extract_root, &dest, &mut created, &mut dir_cache)
                        .map_err(|error| {
                            tar_error("write_failed", format!("Cannot create directory: {error}"))
                        })?;
                }
                #[cfg(not(windows))]
                {
                    fs::create_dir_all(&dest).map_err(|error| {
                        tar_error("write_failed", format!("Cannot create directory: {error}"))
                    })?;
                }
                extracted = extracted.saturating_add(1);
                continue;
            }

            #[cfg(windows)]
            let written = extract_one_file_windows(
                &root,
                extract_root,
                &dest,
                &name,
                &mut entry,
                operation_id,
                cancelled,
                conflict_resolver,
                &mut created,
                &mut dir_cache,
            )?;
            #[cfg(not(windows))]
            let written = extract_one_file_windows(
                extract_root,
                &dest,
                &name,
                &mut entry,
                operation_id,
                cancelled,
                conflict_resolver,
                &mut created,
                &mut dir_cache,
            )?;
            if written {
                extracted = extracted.saturating_add(1);
            } else {
                skipped = skipped.saturating_add(1);
                // Skip leaves unread body — drain so the next tar member stays aligned.
                discard_tar_entry(&mut entry)?;
            }
        }
        Ok(())
    })();

    // Release cached directory handles before cleanup so DELETE disposition can complete.
    #[cfg(windows)]
    drop(dir_cache);

    if let Err(mut error) = result {
        #[cfg(windows)]
        {
            let cleanup_failures = cleanup_windows_created(&mut created);
            if !cleanup_failures.is_empty() {
                error.message.push_str(&format!(
                    " Cleanup issues: {}.",
                    cleanup_failures.join("; ")
                ));
            }
        }
        return Err(error);
    }

    // Selection validity was checked before any write (see prevalidate_tar_selection).
    if planned_or_extracted == 0 {
        return Err(tar_error(
            "empty_selection",
            "No matching archive entries to extract.",
        ));
    }

    let total_files = planned_or_extracted;
    emit(OperationProgress {
        operation_id: operation_id.into(),
        extracted_files: extracted,
        total_files,
        current_file: "Completed".into(),
        percentage: 100.0,
        phase: None,
    });

    Ok(OperationSummary {
        operation_id: operation_id.into(),
        extracted_files: extracted,
        total_files,
        skipped_files: skipped,
        destination: extract_root.to_string_lossy().into_owned(),
    })
}

pub fn extract_tar(
    path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(tar_error("invalid_operation", "Operation ID is empty."));
    }
    if !path.is_file() {
        return Err(tar_error("not_found", "Source archive does not exist."));
    }
    if !destination.is_dir() {
        return Err(tar_error(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }
    let destination = destination.canonicalize().map_err(|error| {
        tar_error(
            "invalid_destination",
            format!("Cannot resolve destination: {error}"),
        )
    })?;
    if let Some(sel) = selected_paths {
        let file = File::open(path)
            .map_err(|error| tar_error("invalid_archive", format!("Cannot open tar: {error}")))?;
        prevalidate_tar_selection(Archive::new(file), sel, cancelled)?;
    }
    let file = File::open(path)
        .map_err(|error| tar_error("invalid_archive", format!("Cannot open tar: {error}")))?;
    let archive = Archive::new(file);
    extract_tar_reader(
        archive,
        &destination,
        operation_id,
        cancelled,
        selected_paths,
        conflict_resolver,
        emit,
    )
}

pub fn extract_tar_gz(
    path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(tar_error("invalid_operation", "Operation ID is empty."));
    }
    if !path.is_file() {
        return Err(tar_error("not_found", "Source archive does not exist."));
    }
    if !destination.is_dir() {
        return Err(tar_error(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }
    let destination = destination.canonicalize().map_err(|error| {
        tar_error(
            "invalid_destination",
            format!("Cannot resolve destination: {error}"),
        )
    })?;
    if let Some(sel) = selected_paths {
        let file = File::open(path).map_err(|error| {
            tar_error("invalid_archive", format!("Cannot open tar.gz: {error}"))
        })?;
        prevalidate_tar_selection(Archive::new(GzDecoder::new(file)), sel, cancelled)?;
    }
    let file = File::open(path)
        .map_err(|error| tar_error("invalid_archive", format!("Cannot open tar.gz: {error}")))?;
    let decoder = GzDecoder::new(file);
    let archive = Archive::new(decoder);
    extract_tar_reader(
        archive,
        &destination,
        operation_id,
        cancelled,
        selected_paths,
        conflict_resolver,
        emit,
    )
}

pub fn extract_tar_bz2(
    path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(tar_error("invalid_operation", "Operation ID is empty."));
    }
    if !path.is_file() {
        return Err(tar_error("not_found", "Source archive does not exist."));
    }
    if !destination.is_dir() {
        return Err(tar_error(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }
    let destination = destination.canonicalize().map_err(|error| {
        tar_error(
            "invalid_destination",
            format!("Cannot resolve destination: {error}"),
        )
    })?;
    if let Some(sel) = selected_paths {
        let file = File::open(path).map_err(|error| {
            tar_error("invalid_archive", format!("Cannot open tar.bz2: {error}"))
        })?;
        prevalidate_tar_selection(Archive::new(BzDecoder::new(file)), sel, cancelled)?;
    }
    let file = File::open(path)
        .map_err(|error| tar_error("invalid_archive", format!("Cannot open tar.bz2: {error}")))?;
    let decoder = BzDecoder::new(file);
    let archive = Archive::new(decoder);
    extract_tar_reader(
        archive,
        &destination,
        operation_id,
        cancelled,
        selected_paths,
        conflict_resolver,
        emit,
    )
}

pub fn extract_tar_xz(
    path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(tar_error("invalid_operation", "Operation ID is empty."));
    }
    if !path.is_file() {
        return Err(tar_error("not_found", "Source archive does not exist."));
    }
    if !destination.is_dir() {
        return Err(tar_error(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }
    let destination = destination.canonicalize().map_err(|error| {
        tar_error(
            "invalid_destination",
            format!("Cannot resolve destination: {error}"),
        )
    })?;
    if let Some(sel) = selected_paths {
        let file = File::open(path).map_err(|error| {
            tar_error("invalid_archive", format!("Cannot open tar.xz: {error}"))
        })?;
        prevalidate_tar_selection(Archive::new(XzDecoder::new(file)), sel, cancelled)?;
    }
    let file = File::open(path)
        .map_err(|error| tar_error("invalid_archive", format!("Cannot open tar.xz: {error}")))?;
    let decoder = XzDecoder::new(file);
    let archive = Archive::new(decoder);
    extract_tar_reader(
        archive,
        &destination,
        operation_id,
        cancelled,
        selected_paths,
        conflict_resolver,
        emit,
    )
}
