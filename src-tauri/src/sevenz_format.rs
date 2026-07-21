//! 7z open/list/extract and create (LZMA2, solid for max ratio).

#[cfg(windows)]
use crate::conflict::unique_renamed_path;
use crate::create_common::{
    cancelled_error, cleanup_temp, create_error, create_temporary_archive, enumerate_sources,
    member_path_for_tar, open_source_file, progress_percentage, publish_temp_archive,
    revalidate_source_entry, validate_sources_and_output, ProgressGate, PROGRESS_INTERVAL,
};
use crate::extraction::{validate_selection, ConflictResolver, SelectionIndex};
use crate::models::{
    ArchiveCapabilities, ArchiveEntry, ArchiveInfo, ArchiveStats, CommandError, CompressionPreset,
    ConflictDecision, CreateOptions, OperationProgress, OperationSummary,
};
use crate::security::{
    assess_archive, is_link_or_reparse_point, safe_destination_path_under_canonical,
    validate_entry_path, ArchiveRiskInput,
};
#[cfg(windows)]
use crate::windows_fs::{cleanup_created as cleanup_windows_created, Directory};
use sevenz_rust2::encoder_options::Lzma2Options;
use sevenz_rust2::{ArchiveEntry as SzEntry, ArchiveReader, ArchiveWriter, Password};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::io_perf::IO_BUFFER_SIZE as BUFFER_SIZE;

fn sz_cb_err(msg: impl Into<String>) -> sevenz_rust2::Error {
    sevenz_rust2::Error::Other(msg.into().into())
}

fn sz_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn map_sz_error(error: sevenz_rust2::Error) -> CommandError {
    use sevenz_rust2::Error as E;
    match &error {
        E::PasswordRequired | E::MaybeBadPassword(_) => sz_error(
            "password_required",
            "Encrypted 7z archives are not supported yet. Open an unencrypted archive.",
        ),
        _ => {
            let message = error.to_string();
            let lower = message.to_ascii_lowercase();
            if lower.contains("password") || lower.contains("encrypt") {
                return sz_error(
                    "password_required",
                    "Encrypted 7z archives are not supported yet. Open an unencrypted archive.",
                );
            }
            sz_error("invalid_archive", format!("7z error: {message}"))
        }
    }
}

fn read_only_open_capabilities() -> ArchiveCapabilities {
    ArchiveCapabilities {
        open: true,
        list: true,
        extract: true,
        create: false,
        edit: true,
        encrypt: false,
        test: true,
    }
}

fn normalize_member_name(raw: &str) -> Result<String, CommandError> {
    let mut normalized = raw.replace('\\', "/");
    while normalized.starts_with("./") {
        normalized = normalized[2..].to_string();
    }
    normalized = normalized.trim_matches('/').to_string();
    if normalized.is_empty() {
        return Err(sz_error("invalid_entry", "Archive entry path is empty."));
    }
    validate_entry_path(&normalized).map_err(|message| CommandError {
        code: "invalid_entry".into(),
        message,
        path: Some(normalized.clone()),
    })?;
    Ok(normalized)
}

/// Open a 7z archive for listing.
pub fn open_sevenz(path: &Path) -> Result<ArchiveInfo, CommandError> {
    if !path.is_file() {
        return Err(sz_error("not_found", "File not found or is not a file."));
    }
    let on_disk = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let reader = ArchiveReader::open(path, Password::empty()).map_err(map_sz_error)?;
    let archive = reader.archive();

    // Virtual parents roughly double entry count in deep trees; reserve modestly.
    let reserve = archive.files.len().saturating_mul(2).max(16);
    let mut entries: Vec<ArchiveEntry> = Vec::with_capacity(reserve);
    let mut entry_indices: HashMap<String, usize> = HashMap::with_capacity(reserve);
    let mut total_uncompressed: u64 = 0;
    let mut total_compressed_members: u64 = 0;
    let mut largest_entry: u64 = 0;
    let mut deepest_path = 0_usize;
    let mut physical = 0_usize;

    for file in &archive.files {
        if file.is_anti_item {
            continue;
        }
        let normalized = match normalize_member_name(file.name()) {
            Ok(n) => n,
            Err(err) => return Err(err),
        };
        physical = physical.saturating_add(1);
        if !file.is_directory {
            total_uncompressed = total_uncompressed.saturating_add(file.size);
            total_compressed_members =
                total_compressed_members.saturating_add(file.compressed_size);
            largest_entry = largest_entry.max(file.size);
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
            let component_is_dir = j < parts.len() - 1 || file.is_directory;

            if let Some(&entry_index) = entry_indices.get(&current_prefix) {
                if j == parts.len() - 1 && file.is_directory {
                    let entry = &mut entries[entry_index];
                    entry.is_directory = true;
                    entry.uncompressed_size = 0;
                    entry.compressed_size = None;
                    entry.method = None;
                }
            } else {
                let uncompressed_size = if component_is_dir { 0 } else { file.size };
                let compressed_size = if component_is_dir {
                    None
                } else if file.compressed_size > 0 {
                    Some(file.compressed_size)
                } else {
                    None
                };
                entries.push(ArchiveEntry {
                    path: current_prefix.clone(),
                    name: (*part).to_string(),
                    parent_path: parent,
                    is_directory: component_is_dir,
                    uncompressed_size,
                    compressed_size,
                    modified_at: None,
                    method: (!component_is_dir).then(|| "LZMA2/7z".into()),
                });
                entry_indices.insert(current_prefix.clone(), entries.len() - 1);
            }
        }
    }

    // Solid archives often leave per-file compressed_size 0; allocate on-disk share.
    if total_compressed_members == 0 && total_uncompressed > 0 {
        allocate_packed(&mut entries, on_disk, total_uncompressed);
    }

    let mut file_count = 0_u64;
    let mut folder_count = 0_u64;
    let mut methods = BTreeSet::new();
    for entry in &entries {
        if entry.is_directory {
            folder_count += 1;
        } else {
            file_count += 1;
            if let Some(m) = &entry.method {
                methods.insert(m.clone());
            }
        }
    }

    // Prefer real on-disk archive size for packed total (headers + streams).
    let total_compressed = if on_disk > 0 {
        on_disk
    } else {
        total_compressed_members
    };

    let entry_count = physical.max(entries.len());
    Ok(ArchiveInfo {
        archive_path: path.to_string_lossy().into_owned(),
        format: "7z".into(),
        entries,
        capabilities: read_only_open_capabilities(),
        warnings: assess_archive(ArchiveRiskInput {
            entry_count,
            total_uncompressed,
            total_compressed,
            largest_entry,
            deepest_path,
        }),
        stats: ArchiveStats {
            file_count,
            folder_count,
            total_uncompressed,
            total_compressed,
            methods: methods.into_iter().collect(),
        },
    })
}

fn allocate_packed(entries: &mut [ArchiveEntry], packed_total: u64, total_uncompressed: u64) {
    let idxs: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.is_directory)
        .map(|(i, _)| i)
        .collect();
    if idxs.is_empty() || total_uncompressed == 0 {
        return;
    }
    let mut assigned = 0_u64;
    for (k, &i) in idxs.iter().enumerate() {
        let unc = entries[i].uncompressed_size;
        let share = if k + 1 == idxs.len() {
            packed_total.saturating_sub(assigned)
        } else {
            let s = ((packed_total as u128)
                .saturating_mul(unc as u128)
                .saturating_div(total_uncompressed as u128)) as u64;
            assigned = assigned.saturating_add(s);
            s
        };
        entries[i].compressed_size = Some(share);
    }
}

/// Extract 7z with path validation and secure destination writes.
pub fn extract_sevenz(
    path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    mut emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(sz_error("invalid_operation", "Operation ID is empty."));
    }
    if !path.is_file() {
        return Err(sz_error("not_found", "Source archive does not exist."));
    }
    if !destination.is_dir() {
        return Err(sz_error(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }
    let destination = destination.canonicalize().map_err(|error| {
        sz_error(
            "invalid_destination",
            format!("Cannot resolve destination: {error}"),
        )
    })?;

    let mut reader = ArchiveReader::open(path, Password::empty()).map_err(map_sz_error)?;
    let names: Vec<String> = reader
        .archive()
        .files
        .iter()
        .filter(|f| !f.is_anti_item)
        .filter_map(|f| normalize_member_name(f.name()).ok())
        .collect();

    let selection_index = match selected_paths {
        Some(sel) if sel.is_empty() => {
            return Err(sz_error(
                "empty_selection",
                "No archive entries were selected for extraction.",
            ));
        }
        Some(sel) => {
            validate_selection(sel, &names)?;
            Some(SelectionIndex::from_selected(sel)?)
        }
        None => None,
    };

    // Note: solid 7z may still decode skipped members; SelectionIndex only avoids O(n×m) string work.
    let total_files = match &selection_index {
        Some(idx) => names.iter().filter(|n| idx.includes_normalized(n)).count() as u64,
        None => names.len() as u64,
    }
    .max(1);

    let mut extracted = 0_u64;
    let mut skipped = 0_u64;
    let mut processed = 0_u64;
    let mut last = Instant::now();

    #[cfg(windows)]
    let mut created = Vec::new();
    #[cfg(windows)]
    let mut dir_cache = HashMap::new();
    #[cfg(not(windows))]
    let mut created: Vec<()> = Vec::new();
    #[cfg(not(windows))]
    let mut dir_cache: HashMap<PathBuf, ()> = HashMap::new();

    // Open destination root once per extract (mirrors ZIP extract_windows).
    #[cfg(windows)]
    let root = Directory::open_root(&destination).map_err(|error| {
        sz_error(
            "unsafe_destination",
            format!("Cannot open destination root: {error}"),
        )
    })?;

    let result = reader.for_each_entries(|entry, data| {
        if cancelled.load(Ordering::Relaxed) {
            return Err(sz_cb_err("cancelled"));
        }
        if entry.is_anti_item {
            return Ok(true);
        }
        let normalized = match normalize_member_name(entry.name()) {
            Ok(n) => n,
            Err(e) => return Err(sz_cb_err(e.message)),
        };

        // `normalized` is already normalize_entry_name'd via normalize_member_name.
        let include = match &selection_index {
            None => true,
            Some(idx) => idx.includes_normalized(&normalized),
        };

        if !include {
            // Drain stream for solid archives so later members can decode.
            let mut sink = [0_u8; BUFFER_SIZE];
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(sz_cb_err("cancelled"));
                }
                match data.read(&mut sink) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            return Ok(true);
        }

        if !emitted_recent(&mut last) {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: extracted,
                total_files,
                current_file: normalized.clone(),
                percentage: progress_percentage(processed, total_files),
                phase: None,
            });
        }

        // `destination` is already canonical at extract entry.
        let dest = match safe_destination_path_under_canonical(&destination, &normalized) {
            Ok(p) => p,
            Err(message) => return Err(sz_cb_err(message)),
        };

        if entry.is_directory {
            #[cfg(windows)]
            {
                // Ensure the directory itself (not only its parent), matching ZIP/TAR.
                root.ensure_path(&destination, &dest, &mut created, &mut dir_cache)
                    .map_err(|e| sz_cb_err(format!("Cannot create directory: {e}")))?;
            }
            #[cfg(not(windows))]
            {
                fs::create_dir_all(&dest)
                    .map_err(|e| sz_cb_err(format!("Cannot create directory: {e}")))?;
            }
            extracted = extracted.saturating_add(1);
            processed = processed.saturating_add(1);
            return Ok(true);
        }

        #[cfg(windows)]
        let write_result = write_extracted_file(
            &root,
            &destination,
            &dest,
            &normalized,
            data,
            operation_id,
            cancelled,
            conflict_resolver,
            &mut created,
            &mut dir_cache,
        );
        #[cfg(not(windows))]
        let write_result = write_extracted_file(
            &destination,
            &dest,
            &normalized,
            data,
            operation_id,
            cancelled,
            conflict_resolver,
            &mut created,
            &mut dir_cache,
        );
        match write_result {
            Ok(true) => extracted = extracted.saturating_add(1),
            Ok(false) => skipped = skipped.saturating_add(1),
            Err(e) if e.code == "cancelled" => return Err(sz_cb_err("cancelled")),
            Err(e) => return Err(sz_cb_err(e.message)),
        }
        processed = processed.saturating_add(1);
        Ok(true)
    });

    // Release cached directory handles before cleanup so DELETE disposition can complete.
    #[cfg(windows)]
    drop(dir_cache);

    if let Err(error) = result {
        #[cfg(windows)]
        {
            let cleanup_failures = cleanup_windows_created(&mut created);
            let mut err = if error.to_string().contains("cancelled") {
                sz_error("cancelled", "Archive extraction was cancelled.")
            } else {
                map_sz_error(error)
            };
            if !cleanup_failures.is_empty() {
                err.message.push_str(&format!(
                    " Cleanup issues: {}.",
                    cleanup_failures.join("; ")
                ));
            }
            return Err(err);
        }
        #[cfg(not(windows))]
        {
            let msg = error.to_string();
            if msg.contains("cancelled") {
                return Err(sz_error("cancelled", "Archive extraction was cancelled."));
            }
            return Err(map_sz_error(error));
        }
    }

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
        destination: destination.to_string_lossy().into_owned(),
    })
}

fn emitted_recent(last: &mut Instant) -> bool {
    if last.elapsed() < PROGRESS_INTERVAL {
        true
    } else {
        *last = Instant::now();
        false
    }
}

#[cfg(windows)]
fn write_extracted_file(
    root: &Directory,
    extract_root: &Path,
    destination: &Path,
    entry_path: &str,
    mut reader: impl Read,
    operation_id: &str,
    cancelled: &AtomicBool,
    conflict_resolver: &dyn ConflictResolver,
    created: &mut Vec<crate::windows_fs::CreatedEntry>,
    dir_cache: &mut HashMap<PathBuf, Directory>,
) -> Result<bool, CommandError> {
    use std::os::windows::ffi::OsStrExt;

    let mut write_to = destination.to_path_buf();
    loop {
        match fs::symlink_metadata(&write_to) {
            Ok(meta) => {
                if is_link_or_reparse_point(&meta) {
                    return Err(sz_error(
                        "unsafe_destination",
                        "Destination path is a reparse point.",
                    ));
                }
                if meta.is_dir() {
                    return Err(sz_error(
                        "conflict",
                        "Cannot overwrite a directory with a file.",
                    ));
                }
                let decision =
                    conflict_resolver.resolve_file_exists(operation_id, entry_path, &write_to)?;
                match decision {
                    ConflictDecision::Skip => return Ok(false),
                    ConflictDecision::Cancel => {
                        return Err(sz_error("cancelled", "Archive extraction was cancelled."))
                    }
                    ConflictDecision::Overwrite => {
                        fs::remove_file(&write_to).map_err(|error| {
                            sz_error(
                                "write_failed",
                                format!("Cannot remove existing file: {error}"),
                            )
                        })?;
                        break;
                    }
                    ConflictDecision::Rename => {
                        let parent = write_to.parent().ok_or_else(|| {
                            sz_error("invalid_destination", "Destination has no parent.")
                        })?;
                        let file_name = write_to
                            .file_name()
                            .ok_or_else(|| {
                                sz_error("invalid_destination", "Destination has no file name.")
                            })?
                            .to_string_lossy();
                        write_to = unique_renamed_path(parent, &file_name)?;
                        continue;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(sz_error(
                    "write_failed",
                    format!("Cannot inspect destination: {error}"),
                ));
            }
        }
    }

    let parent = root
        .parent_for(extract_root, &write_to, created, dir_cache)
        .map_err(|error| {
            sz_error(
                "write_failed",
                format!("Cannot create destination parents: {error}"),
            )
        })?;
    let file_name = write_to
        .file_name()
        .ok_or_else(|| sz_error("invalid_destination", "Destination has no file name."))?;
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
        .map_err(|error| sz_error("write_failed", format!("Cannot create temp file: {error}")))?;
    let mut buffer = [0_u8; BUFFER_SIZE];
    {
        let mut writer = output.as_ref();
        loop {
            if cancelled.load(Ordering::Relaxed) {
                drop(output);
                let _ = cleanup_windows_created(created);
                return Err(sz_error("cancelled", "Archive extraction was cancelled."));
            }
            let n = reader.read(&mut buffer).map_err(|error| {
                sz_error("invalid_archive", format!("Cannot read 7z member: {error}"))
            })?;
            if n == 0 {
                break;
            }
            writer.write_all(&buffer[..n]).map_err(|error| {
                sz_error(
                    "write_failed",
                    format!("Cannot write extracted file: {error}"),
                )
            })?;
        }
        writer
            .flush()
            .map_err(|error| sz_error("write_failed", format!("Cannot flush file: {error}")))?;
    }
    drop(output);
    let created_file = created
        .get(created_index)
        .ok_or_else(|| sz_error("write_failed", "Missing created temp file handle."))?;
    parent
        .rename_new_file(created_file, &wide)
        .map_err(|error| {
            sz_error(
                "write_failed",
                format!("Cannot finalize extracted file: {error}"),
            )
        })?;
    Ok(true)
}

#[cfg(not(windows))]
fn write_extracted_file(
    _extract_root: &Path,
    destination: &Path,
    entry_path: &str,
    mut reader: impl Read,
    operation_id: &str,
    cancelled: &AtomicBool,
    conflict_resolver: &dyn ConflictResolver,
    _created: &mut Vec<()>,
    _dir_cache: &mut HashMap<PathBuf, ()>,
) -> Result<bool, CommandError> {
    let mut write_to = destination.to_path_buf();
    if write_to.exists() {
        let decision =
            conflict_resolver.resolve_file_exists(operation_id, entry_path, &write_to)?;
        match decision {
            ConflictDecision::Skip => return Ok(false),
            ConflictDecision::Cancel => {
                return Err(sz_error("cancelled", "Archive extraction was cancelled."))
            }
            ConflictDecision::Overwrite => {
                fs::remove_file(&write_to).map_err(|e| {
                    sz_error("write_failed", format!("Cannot remove existing file: {e}"))
                })?;
            }
            ConflictDecision::Rename => {
                return Err(sz_error(
                    "unsupported_operation",
                    "Rename conflict is Windows-only in this build.",
                ));
            }
        }
    }
    if let Some(parent) = write_to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| sz_error("write_failed", format!("Cannot create parents: {e}")))?;
    }
    let mut file = fs::File::create(&write_to)
        .map_err(|e| sz_error("write_failed", format!("Cannot create file: {e}")))?;
    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(sz_error("cancelled", "Archive extraction was cancelled."));
        }
        let n = reader
            .read(&mut buffer)
            .map_err(|e| sz_error("invalid_archive", format!("Cannot read: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])
            .map_err(|e| sz_error("write_failed", format!("Cannot write: {e}")))?;
    }
    Ok(true)
}

fn lzma2_level(preset: CompressionPreset) -> u32 {
    match preset {
        CompressionPreset::Store => 0,
        CompressionPreset::Fast => 3,
        CompressionPreset::Normal => 5,
        // Maximum dictionary/effort for product 7z create.
        CompressionPreset::Max => 9,
    }
}

/// Create a 7z archive. Uses solid LZMA2 for Normal/Max (best ratio); non-solid for Store/Fast.
pub fn create_sevenz_archive(
    source_paths: &[String],
    output_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    options: &CreateOptions,
    mut emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(create_error("invalid_operation", "Operation ID is empty."));
    }
    if source_paths.is_empty() {
        return Err(create_error("invalid_source", "No source files specified."));
    }

    let (output_path, source_paths) =
        validate_sources_and_output(source_paths, output_path, options.overwrite)?;
    let entries = enumerate_sources(&source_paths, options.include_root, cancelled)?;
    let total_files = entries.len() as u64;
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let (temp_path, temp_file) = create_temporary_archive(&output_path)?;
    // Max compression = LZMA2 level 9 (dictionary/effort). Per-file streams keep
    // cancel responsive; solid packing can be added later if needed for tiny gains.
    let level = lzma2_level(options.compression);

    let result = (|| -> Result<OperationSummary, CommandError> {
        let mut writer = ArchiveWriter::new(temp_file).map_err(map_sz_error)?;
        writer.set_content_methods(vec![Lzma2Options::from_level(level).into()]);

        let mut processed = 0_u64;
        let mut progress_gate = ProgressGate::new();

        for entry in &entries {
            if cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }
            revalidate_source_entry(entry)?;
            let member = member_path_for_tar(&entry.archive_path);
            if member.is_empty() {
                return Err(create_error(
                    "invalid_source",
                    "Archive member path is empty.",
                ));
            }
            // First entry always; mid-entries at most every PROGRESS_INTERVAL; final 100% outside loop.
            if progress_gate.should_emit() {
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files: processed,
                    total_files,
                    current_file: entry.archive_path.clone(),
                    percentage: progress_percentage(processed, total_files),
                    phase: None,
                });
            }
            if entry.is_directory {
                writer
                    .push_archive_entry(SzEntry::new_directory(&member), None::<File>)
                    .map_err(map_sz_error)?;
            } else {
                let source = open_source_file(&entry.path).map_err(|error| {
                    create_error(
                        "source_read",
                        format!("Cannot open source {}: {error}", entry.path.display()),
                    )
                })?;
                writer
                    .push_archive_entry(SzEntry::from_path(&entry.path, member), Some(source))
                    .map_err(map_sz_error)?;
            }
            processed = processed.saturating_add(1);
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let finished = writer.finish().map_err(|error| {
            create_error(
                "write_failed",
                format!("Cannot finalize 7z archive: {error}"),
            )
        })?;
        finished.sync_all().map_err(|error| {
            create_error("write_failed", format!("Cannot sync 7z archive: {error}"))
        })?;
        drop(finished);

        publish_temp_archive(&temp_path, &output_path, options.overwrite).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                create_error(
                    "output_exists",
                    "Output archive appeared before finalization.",
                )
            } else {
                create_error(
                    "finalize_failed",
                    format!("Cannot finalize output archive: {error}"),
                )
            }
        })?;

        Ok(OperationSummary {
            operation_id: operation_id.into(),
            extracted_files: processed,
            total_files,
            skipped_files: 0,
            destination: output_path.to_string_lossy().into_owned(),
        })
    })();

    match result {
        Ok(summary) => {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: summary.extracted_files,
                total_files: summary.total_files,
                current_file: "Completed".into(),
                percentage: 100.0,
                phase: None,
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}
