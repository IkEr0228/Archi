//! Single-stream GZIP open/list/extract (not tar.gz).

#[cfg(windows)]
use crate::conflict::unique_renamed_path;
use crate::extraction::ConflictResolver;
use crate::io_perf::{IO_BUFFER_SIZE as BUFFER_SIZE, PROGRESS_INTERVAL};
use crate::models::{
    ArchiveCapabilities, ArchiveEntry, ArchiveInfo, ArchiveStats, CommandError, ConflictDecision,
    OperationProgress, OperationSummary,
};
use crate::security::{
    assess_archive, is_link_or_reparse_point, validate_entry_path, ArchiveRiskInput,
};
#[cfg(windows)]
use crate::windows_fs::{cleanup_created as cleanup_windows_created, Directory};
use flate2::read::GzDecoder;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// Gzip ISIZE trailer: last 4 bytes of a member, little-endian uncompressed length
/// **modulo 2^32**. For payloads ≥ 4 GiB the reported size wraps; extract still
/// streams the full payload and is the source of truth for bytes written.
fn gzip_isize_from_trailer(path: &Path, on_disk: u64) -> Result<u64, CommandError> {
    if on_disk < 8 {
        return Ok(0);
    }
    let mut file = File::open(path)
        .map_err(|error| gzip_error("invalid_archive", format!("Cannot open gzip: {error}")))?;
    file.seek(SeekFrom::End(-4)).map_err(|error| {
        gzip_error(
            "invalid_archive",
            format!("Cannot seek gzip ISIZE trailer: {error}"),
        )
    })?;
    let mut buf = [0_u8; 4];
    file.read_exact(&mut buf).map_err(|error| {
        gzip_error(
            "invalid_archive",
            format!("Cannot read gzip ISIZE trailer: {error}"),
        )
    })?;
    Ok(u32::from_le_bytes(buf) as u64)
}

fn gzip_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn read_only_capabilities() -> ArchiveCapabilities {
    ArchiveCapabilities {
        open: true,
        list: true,
        extract: true,
        create: false,
        // Single-stream: integrity test only (no multi-entry edit).
        edit: false,
        encrypt: false,
        test: true,
    }
}

/// Logical entry name for a single gzip payload.
pub fn gzip_entry_name(archive_path: &Path) -> Result<String, CommandError> {
    let file = File::open(archive_path)
        .map_err(|error| gzip_error("invalid_archive", format!("Cannot open gzip: {error}")))?;
    let decoder = GzDecoder::new(file);
    if let Some(name) = decoder.header().and_then(|h| h.filename()) {
        let raw = String::from_utf8_lossy(name).replace('\\', "/");
        let base = raw
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(raw.as_str())
            .trim_matches('/');
        if !base.is_empty() && validate_entry_path(base).is_ok() {
            return Ok(base.to_string());
        }
    }
    let file_name = archive_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".into());
    let stem = if let Some(stripped) = file_name
        .strip_suffix(".gz")
        .or_else(|| file_name.strip_suffix(".GZ"))
    {
        if stripped.is_empty() {
            "data".into()
        } else {
            stripped.to_string()
        }
    } else {
        format!("{file_name}.out")
    };
    validate_entry_path(&stem).map_err(|message| CommandError {
        code: "invalid_entry".into(),
        message,
        path: Some(stem.clone()),
    })?;
    Ok(stem)
}

pub fn open_gzip(path: &Path) -> Result<ArchiveInfo, CommandError> {
    if !path.is_file() {
        return Err(gzip_error("not_found", "File not found or is not a file."));
    }
    let name = gzip_entry_name(path)?;
    let on_disk = fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    // Use gzip ISIZE trailer (last 4 bytes) — no full inflate on open.
    // ISIZE is uncompressed size mod 2^32; extract remains authoritative for ≥4 GiB.
    let size = gzip_isize_from_trailer(path, on_disk)?;

    let entries = vec![ArchiveEntry {
        path: name.clone(),
        name: name.clone(),
        parent_path: "/".into(),
        is_directory: false,
        uncompressed_size: size,
        compressed_size: Some(on_disk),
        modified_at: None,
        method: Some("Gzip".into()),
    }];

    Ok(ArchiveInfo {
        archive_path: path.to_string_lossy().into_owned(),
        format: "gzip".into(),
        entries,
        capabilities: read_only_capabilities(),
        warnings: assess_archive(ArchiveRiskInput {
            entry_count: 1,
            total_uncompressed: size,
            total_compressed: on_disk,
            largest_entry: size,
            deepest_path: 1,
        }),
        stats: ArchiveStats {
            file_count: 1,
            folder_count: 0,
            total_uncompressed: size,
            total_compressed: on_disk,
            methods: vec!["Gzip".into()],
        },
    })
}

pub fn extract_gzip(
    path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    mut emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(gzip_error("invalid_operation", "Operation ID is empty."));
    }
    if !path.is_file() {
        return Err(gzip_error("not_found", "Source archive does not exist."));
    }
    if !destination.is_dir() {
        return Err(gzip_error(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }
    let destination = destination.canonicalize().map_err(|error| {
        gzip_error(
            "invalid_destination",
            format!("Cannot resolve destination: {error}"),
        )
    })?;

    let name = gzip_entry_name(path)?;
    if let Some(sel) = selected_paths {
        if sel.is_empty() {
            return Err(gzip_error(
                "empty_selection",
                "No archive entries were selected for extraction.",
            ));
        }
        let ok = sel.iter().any(|s| {
            let n = s.trim_matches('/');
            n == name || n == name.as_str()
        });
        if !ok {
            return Err(gzip_error(
                "empty_selection",
                "Selection does not match the gzip entry.",
            ));
        }
    }

    emit(OperationProgress {
        operation_id: operation_id.into(),
        extracted_files: 0,
        total_files: 1,
        current_file: name.clone(),
        percentage: 0.0,
        phase: None,
    });

    // Resolve destination conflicts before decompressing (skip avoids full inflate).
    let dest = destination.join(&name);
    let mut write_to = dest;
    let mut skipped = 0_u64;
    let mut extracted = 0_u64;

    loop {
        match fs::symlink_metadata(&write_to) {
            Ok(meta) => {
                if is_link_or_reparse_point(&meta) {
                    return Err(gzip_error(
                        "unsafe_destination",
                        "Destination path is a reparse point.",
                    ));
                }
                if meta.is_dir() {
                    return Err(gzip_error(
                        "conflict",
                        "Cannot overwrite a directory with a file.",
                    ));
                }
                let decision =
                    conflict_resolver.resolve_file_exists(operation_id, &name, &write_to)?;
                match decision {
                    ConflictDecision::Skip => {
                        skipped = 1;
                        break;
                    }
                    ConflictDecision::Cancel => {
                        return Err(gzip_error("cancelled", "Archive extraction was cancelled."))
                    }
                    ConflictDecision::Overwrite => {
                        fs::remove_file(&write_to).map_err(|error| {
                            gzip_error(
                                "write_failed",
                                format!("Cannot remove existing file: {error}"),
                            )
                        })?;
                        break;
                    }
                    ConflictDecision::Rename => {
                        #[cfg(windows)]
                        {
                            let parent = write_to.parent().unwrap_or(&destination);
                            let file_name = write_to
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| name.clone());
                            write_to = unique_renamed_path(parent, &file_name)?;
                            continue;
                        }
                        #[cfg(not(windows))]
                        {
                            return Err(gzip_error(
                                "unsupported_operation",
                                "Rename conflict is Windows-only in this build.",
                            ));
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(gzip_error(
                    "write_failed",
                    format!("Cannot inspect destination: {error}"),
                ));
            }
        }
    }

    if skipped == 0 {
        let file = File::open(path)
            .map_err(|error| gzip_error("invalid_archive", format!("Cannot open gzip: {error}")))?;
        let mut decoder = GzDecoder::new(file);
        let mut buffer = [0_u8; BUFFER_SIZE];
        let mut last = Instant::now();

        #[cfg(windows)]
        {
            let mut created = Vec::new();
            let mut dir_cache = std::collections::HashMap::new();
            let root = Directory::open_root(&destination).map_err(|error| {
                gzip_error(
                    "unsafe_destination",
                    format!("Cannot open destination root: {error}"),
                )
            })?;
            let parent = root
                .parent_for(&destination, &write_to, &mut created, &mut dir_cache)
                .map_err(|error| {
                    gzip_error(
                        "write_failed",
                        format!("Cannot create destination parents: {error}"),
                    )
                })?;
            use std::os::windows::ffi::OsStrExt;
            let file_name = write_to.file_name().ok_or_else(|| {
                gzip_error("invalid_destination", "Destination has no file name.")
            })?;
            let wide: Vec<u16> = file_name.encode_wide().collect();
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
            let created_index = created.len();
            let out = parent
                .create_file(&temp_name, &mut created)
                .map_err(|error| {
                    gzip_error("write_failed", format!("Cannot create temp file: {error}"))
                })?;
            {
                let mut writer = out.as_ref();
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err(gzip_error("cancelled", "Archive extraction was cancelled."));
                    }
                    let n = decoder.read(&mut buffer).map_err(|error| {
                        gzip_error(
                            "invalid_archive",
                            format!("Cannot decompress gzip: {error}"),
                        )
                    })?;
                    if n == 0 {
                        break;
                    }
                    writer.write_all(&buffer[..n]).map_err(|error| {
                        gzip_error("write_failed", format!("Cannot write file: {error}"))
                    })?;
                    if last.elapsed() >= PROGRESS_INTERVAL {
                        last = Instant::now();
                        emit(OperationProgress {
                            operation_id: operation_id.into(),
                            extracted_files: 0,
                            total_files: 1,
                            current_file: name.clone(),
                            percentage: 50.0,
                            phase: None,
                        });
                    }
                }
                writer.flush().map_err(|error| {
                    gzip_error("write_failed", format!("Cannot flush file: {error}"))
                })?;
            }
            drop(out);
            let created_file = created
                .get(created_index)
                .ok_or_else(|| gzip_error("write_failed", "Missing created temp file handle."))?;
            if let Err(error) = parent.rename_new_file(created_file, &wide) {
                let _ = cleanup_windows_created(&mut created);
                return Err(gzip_error(
                    "write_failed",
                    format!("Cannot finalize file: {error}"),
                ));
            }
            extracted = 1;
        }
        #[cfg(not(windows))]
        {
            if let Some(parent) = write_to.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    gzip_error("write_failed", format!("Cannot create parents: {error}"))
                })?;
            }
            let mut out = fs::File::create(&write_to).map_err(|error| {
                gzip_error("write_failed", format!("Cannot create file: {error}"))
            })?;
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(gzip_error("cancelled", "Archive extraction was cancelled."));
                }
                let n = decoder.read(&mut buffer).map_err(|error| {
                    gzip_error(
                        "invalid_archive",
                        format!("Cannot decompress gzip: {error}"),
                    )
                })?;
                if n == 0 {
                    break;
                }
                out.write_all(&buffer[..n]).map_err(|error| {
                    gzip_error("write_failed", format!("Cannot write file: {error}"))
                })?;
                if last.elapsed() >= PROGRESS_INTERVAL {
                    last = Instant::now();
                    emit(OperationProgress {
                        operation_id: operation_id.into(),
                        extracted_files: 0,
                        total_files: 1,
                        current_file: name.clone(),
                        percentage: 50.0,
                        phase: None,
                    });
                }
            }
            extracted = 1;
        }
    }

    emit(OperationProgress {
        operation_id: operation_id.into(),
        extracted_files: extracted,
        total_files: 1,
        current_file: "Completed".into(),
        percentage: 100.0,
        phase: None,
    });

    Ok(OperationSummary {
        operation_id: operation_id.into(),
        extracted_files: extracted,
        total_files: 1,
        skipped_files: skipped,
        destination: destination.to_string_lossy().into_owned(),
    })
}
