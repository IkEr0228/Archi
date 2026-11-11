//! Transactional extraction of single-stream codecs. Skip never opens the decoder.
use crate::conflict::unique_renamed_path;
use crate::extraction::ConflictResolver;
use crate::io_perf::{IO_BUFFER_SIZE, PROGRESS_INTERVAL};
use crate::models::{CommandError, ConflictDecision, OperationProgress, OperationSummary};
use crate::security::{is_link_or_reparse_point, validate_entry_path};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub(crate) fn info(
    path: &Path,
    name: String,
    format: &str,
    method: &str,
    size: u64,
) -> crate::models::ArchiveInfo {
    use crate::models::{ArchiveCapabilities, ArchiveEntry, ArchiveInfo, ArchiveStats};
    let on_disk = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    ArchiveInfo {
        archive_path: path.to_string_lossy().into_owned(),
        format: format.into(),
        entries: vec![ArchiveEntry {
            path: name.clone(),
            name,
            parent_path: "/".into(),
            is_directory: false,
            uncompressed_size: size,
            compressed_size: Some(on_disk),
            modified_at: None,
            method: Some(method.into()),
        }],
        capabilities: ArchiveCapabilities {
            open: true,
            list: true,
            extract: true,
            create: false,
            edit: false,
            encrypt: false,
            test: true,
        },
        warnings: crate::security::assess_archive(crate::security::ArchiveRiskInput {
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
            methods: vec![method.into()],
        },
    }
}

pub(crate) fn decoded_size(mut reader: impl Read) -> Result<u64, CommandError> {
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; IO_BUFFER_SIZE];
    loop {
        let n = reader.read(&mut buffer).map_err(|e| {
            CommandError::new("invalid_archive", format!("Cannot decompress stream: {e}"))
        })?;
        if n == 0 {
            return Ok(size);
        }
        size = size.saturating_add(n as u64);
    }
}

pub(crate) fn extract(
    destination: &Path,
    name: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    resolver: &dyn ConflictResolver,
    mut emit: impl FnMut(OperationProgress),
    open_decoder: impl FnOnce() -> Result<Box<dyn Read>, CommandError>,
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(CommandError::new(
            "invalid_operation",
            "Operation ID is empty.",
        ));
    }
    validate_entry_path(name).map_err(|message| CommandError::new("invalid_entry", message))?;
    if selected_paths.is_some_and(|paths| !paths.iter().any(|p| p.trim_matches('/') == name)) {
        return Err(CommandError::new(
            "empty_selection",
            "Selection does not match the stream entry.",
        ));
    }
    if !destination.is_dir() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination directory does not exist.",
        ));
    }
    let destination = destination
        .canonicalize()
        .map_err(|e| CommandError::new("invalid_destination", e.to_string()))?;
    let progress = |done, percentage| OperationProgress {
        operation_id: operation_id.into(),
        extracted_files: done,
        total_files: 1,
        current_file: if percentage == 100.0 {
            "Completed".into()
        } else {
            name.into()
        },
        percentage,
        phase: None,
    };
    emit(progress(0, 0.0));
    let mut target = destination.join(name);
    let mut replace = false;
    let skipped = loop {
        check_cancelled(cancelled)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) => {
                if is_link_or_reparse_point(&metadata) {
                    return Err(CommandError::new(
                        "unsafe_destination",
                        "Destination is a reparse point.",
                    ));
                }
                if !metadata.is_file() {
                    return Err(CommandError::new(
                        "conflict",
                        "Cannot overwrite a non-file destination.",
                    ));
                }
                match resolver.resolve_file_exists(operation_id, name, &target)? {
                    ConflictDecision::Skip => break true,
                    ConflictDecision::Cancel => return Err(cancelled_error()),
                    ConflictDecision::Overwrite => {
                        replace = true;
                        break false;
                    }
                    ConflictDecision::Rename => {
                        let parent = target.parent().ok_or_else(|| {
                            CommandError::new("invalid_destination", "Destination has no parent.")
                        })?;
                        let leaf = target.file_name().unwrap().to_string_lossy();
                        target = unique_renamed_path(parent, &leaf)?;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break false,
            Err(e) => return Err(CommandError::new("write_failed", e.to_string())),
        }
    };
    if !skipped {
        let mut decoder = open_decoder()?;
        let mut last = Instant::now();
        write_stream(&destination, &target, replace, cancelled, |writer| {
            let mut buffer = vec![0_u8; IO_BUFFER_SIZE];
            loop {
                check_cancelled(cancelled)?;
                let n = decoder.read(&mut buffer).map_err(|e| {
                    CommandError::new("invalid_archive", format!("Cannot decompress stream: {e}"))
                })?;
                if n == 0 {
                    break;
                }
                writer.write_all(&buffer[..n]).map_err(write_error)?;
                if last.elapsed() >= PROGRESS_INTERVAL {
                    last = Instant::now();
                    emit(progress(0, 50.0));
                }
            }
            Ok(())
        })?;
    }
    let extracted = u64::from(!skipped);
    emit(progress(extracted, 100.0));
    Ok(OperationSummary {
        operation_id: operation_id.into(),
        extracted_files: extracted,
        total_files: 1,
        skipped_files: u64::from(skipped),
        destination: destination.to_string_lossy().into_owned(),
    })
}

fn cancelled_error() -> CommandError {
    CommandError::new("cancelled", "Archive extraction was cancelled.")
}
fn check_cancelled(cancelled: &AtomicBool) -> Result<(), CommandError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}
fn write_error(error: std::io::Error) -> CommandError {
    CommandError::new("write_failed", error.to_string())
}

/// Publishes only after the producer reports success (including its final CRC read).
#[cfg(windows)]
pub(crate) fn write_stream(
    root: &Path,
    target: &Path,
    replace: bool,
    cancelled: &AtomicBool,
    produce: impl FnOnce(&mut dyn Write) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    use crate::windows_fs::{cleanup_created, Directory};
    use std::os::windows::ffi::OsStrExt;
    let directory = Directory::open_root(root)
        .map_err(|e| CommandError::new("unsafe_destination", e.to_string()))?;
    let mut created = Vec::new();
    let result: Result<(), CommandError> = (|| {
        let parent = directory
            .parent_for(root, target, &mut created, &mut ahash::AHashMap::new())
            .map_err(write_error)?;
        let temp: Vec<u16> = format!(
            ".archi-part-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
        .encode_utf16()
        .collect();
        let index = created.len();
        let file = parent
            .create_file(&temp, &mut created)
            .map_err(write_error)?;
        produce(&mut file.as_ref())?;
        check_cancelled(cancelled)?;
        file.sync_all().map_err(write_error)?;
        let leaf: Vec<u16> = target
            .file_name()
            .ok_or_else(|| CommandError::new("invalid_destination", "Destination has no leaf."))?
            .encode_wide()
            .collect();
        parent
            .publish_file(&created[index], &leaf, replace)
            .map_err(write_error)?;
        Ok(())
    })();
    if let Err(mut error) = result {
        for failure in cleanup_created(&mut created) {
            error
                .message
                .push_str(&format!(" Cleanup failed: {failure}"));
        }
        Err(error)
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn write_stream(
    _root: &Path,
    target: &Path,
    replace: bool,
    cancelled: &AtomicBool,
    produce: impl FnOnce(&mut dyn Write) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    // Same-directory staging preserves the previous file on codec/cancel errors.
    let (temp, mut file) = crate::create_common::create_temporary_archive(target)?;
    let result = (|| {
        produce(&mut file)?;
        check_cancelled(cancelled)?;
        file.sync_all().map_err(write_error)?;
        if replace {
            fs::rename(&temp, target).map_err(write_error)?;
        } else {
            fs::hard_link(&temp, target).map_err(write_error)?;
            fs::remove_file(&temp).map_err(write_error)?;
        }
        Ok(())
    })();
    drop(file);
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
