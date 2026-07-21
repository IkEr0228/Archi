//! Integrity test (read/decompress only — no extract to user paths).

use crate::format_detect::{detect_format, ArchiveFormat};
use crate::models::{CommandError, OperationProgress, TestArchiveSummary, TestFailure};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tar::Archive;
use xz2::read::XzDecoder;
use zip::ZipArchive;

use crate::io_perf::{IO_BUFFER_SIZE as BUFFER_SIZE, PROGRESS_INTERVAL};
const MAX_FAILURES: usize = 20;

fn test_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn drain_reader(reader: &mut impl Read, cancelled: &AtomicBool) -> Result<(), String> {
    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Dispatch integrity test by content format.
pub fn test_archive(
    archive_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(test_error("invalid_operation", "Operation ID is empty."));
    }
    if !archive_path.is_file() {
        return Err(test_error("not_found", "Source archive does not exist."));
    }

    match detect_format(archive_path)? {
        ArchiveFormat::Zip => test_zip(archive_path, operation_id, cancelled, emit),
        ArchiveFormat::Tar => test_tar_plain(archive_path, operation_id, cancelled, emit),
        ArchiveFormat::TarGz => test_tar_gz(archive_path, operation_id, cancelled, emit),
        ArchiveFormat::TarBz2 => test_tar_bz2(archive_path, operation_id, cancelled, emit),
        ArchiveFormat::TarXz => test_tar_xz(archive_path, operation_id, cancelled, emit),
        ArchiveFormat::Gzip => test_single_stream_gzip(archive_path, operation_id, cancelled, emit),
        ArchiveFormat::Bzip2 => {
            test_single_stream_bzip2(archive_path, operation_id, cancelled, emit)
        }
        ArchiveFormat::Xz => test_single_stream_xz(archive_path, operation_id, cancelled, emit),
        ArchiveFormat::SevenZ => test_sevenz(archive_path, operation_id, cancelled, emit),
    }
}

fn finish_summary(
    operation_id: &str,
    tested: u64,
    tested_ok: u64,
    tested_failed: u64,
    failures: Vec<TestFailure>,
    mut emit: impl FnMut(OperationProgress),
) -> TestArchiveSummary {
    emit(OperationProgress {
        operation_id: operation_id.into(),
        extracted_files: tested,
        total_files: tested.max(1),
        current_file: "Completed".into(),
        percentage: 100.0,
        phase: None,
    });
    TestArchiveSummary {
        operation_id: operation_id.into(),
        total_entries: tested,
        tested_ok,
        tested_failed,
        failures,
    }
}

fn test_zip(
    zip_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let file = File::open(zip_path)
        .map_err(|error| test_error("invalid_archive", format!("Cannot open archive: {error}")))?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        test_error(
            "invalid_archive",
            format!("Cannot read ZIP structure: {error}"),
        )
    })?;

    let progress_total = archive.len() as u64;
    let mut tested_ok = 0_u64;
    let mut tested_failed = 0_u64;
    let mut failures = Vec::new();
    let mut last_progress = Instant::now()
        .checked_sub(PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut tested = 0_u64;

    for index in 0..archive.len() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(test_error("cancelled", "Archive test was cancelled."));
        }
        let mut entry = archive.by_index(index).map_err(|error| {
            test_error("invalid_archive", format!("Cannot read ZIP entry: {error}"))
        })?;
        let name = entry.name().to_string();
        let is_dir = entry.is_dir() || name.ends_with('/') || name.ends_with('\\');
        if is_dir {
            continue;
        }
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: tested,
                total_files: progress_total,
                current_file: name.clone(),
                percentage: if progress_total == 0 {
                    100.0
                } else {
                    tested as f32 * 100.0 / progress_total as f32
                },
                phase: None,
            });
            last_progress = Instant::now();
        }
        match drain_reader(&mut entry, cancelled) {
            Ok(()) => {
                tested += 1;
                tested_ok += 1;
            }
            Err(msg) if msg == "cancelled" => {
                return Err(test_error("cancelled", "Archive test was cancelled."));
            }
            Err(msg) => {
                tested += 1;
                tested_failed += 1;
                if failures.len() < MAX_FAILURES {
                    failures.push(TestFailure {
                        path: name,
                        message: msg,
                    });
                }
            }
        }
    }

    Ok(finish_summary(
        operation_id,
        tested,
        tested_ok,
        tested_failed,
        failures,
        emit,
    ))
}

fn test_tar_reader<R: Read>(
    mut archive: Archive<R>,
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let mut tested_ok = 0_u64;
    let mut tested_failed = 0_u64;
    let mut failures = Vec::new();
    let mut tested = 0_u64;
    let mut last_progress = Instant::now()
        .checked_sub(PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);

    let entries = archive.entries().map_err(|error| {
        test_error(
            "invalid_archive",
            format!("Cannot read tar entries: {error}"),
        )
    })?;

    for entry in entries {
        if cancelled.load(Ordering::Relaxed) {
            return Err(test_error("cancelled", "Archive test was cancelled."));
        }
        let mut entry = entry.map_err(|error| {
            test_error("invalid_archive", format!("Cannot read tar entry: {error}"))
        })?;
        let path = entry
            .path()
            .map_err(|error| test_error("invalid_entry", format!("Invalid tar path: {error}")))?;
        let name = path.to_string_lossy().replace('\\', "/");
        let is_dir = entry.header().entry_type().is_dir() || name.ends_with('/');
        if is_dir {
            continue;
        }
        if last_progress.elapsed() >= PROGRESS_INTERVAL {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: tested,
                total_files: tested.saturating_add(1).max(1),
                current_file: name.clone(),
                percentage: 0.0,
                phase: None,
            });
            last_progress = Instant::now();
        }
        match drain_reader(&mut entry, cancelled) {
            Ok(()) => {
                tested += 1;
                tested_ok += 1;
            }
            Err(msg) if msg == "cancelled" => {
                return Err(test_error("cancelled", "Archive test was cancelled."));
            }
            Err(msg) => {
                tested += 1;
                tested_failed += 1;
                if failures.len() < MAX_FAILURES {
                    failures.push(TestFailure {
                        path: name,
                        message: msg,
                    });
                }
            }
        }
    }

    Ok(finish_summary(
        operation_id,
        tested,
        tested_ok,
        tested_failed,
        failures,
        emit,
    ))
}

fn test_tar_plain(
    path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let file = File::open(path)
        .map_err(|e| test_error("invalid_archive", format!("Cannot open tar: {e}")))?;
    test_tar_reader(Archive::new(file), operation_id, cancelled, emit)
}

fn test_tar_gz(
    path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let file = File::open(path)
        .map_err(|e| test_error("invalid_archive", format!("Cannot open tar.gz: {e}")))?;
    test_tar_reader(
        Archive::new(GzDecoder::new(file)),
        operation_id,
        cancelled,
        emit,
    )
}

fn test_tar_bz2(
    path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let file = File::open(path)
        .map_err(|e| test_error("invalid_archive", format!("Cannot open tar.bz2: {e}")))?;
    test_tar_reader(
        Archive::new(BzDecoder::new(file)),
        operation_id,
        cancelled,
        emit,
    )
}

fn test_tar_xz(
    path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let file = File::open(path)
        .map_err(|e| test_error("invalid_archive", format!("Cannot open tar.xz: {e}")))?;
    test_tar_reader(
        Archive::new(XzDecoder::new(file)),
        operation_id,
        cancelled,
        emit,
    )
}

fn test_single_named(
    operation_id: &str,
    cancelled: &AtomicBool,
    label: &str,
    mut reader: impl Read,
    mut emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    emit(OperationProgress {
        operation_id: operation_id.into(),
        extracted_files: 0,
        total_files: 1,
        current_file: label.into(),
        percentage: 0.0,
        phase: None,
    });
    match drain_reader(&mut reader, cancelled) {
        Ok(()) => Ok(finish_summary(operation_id, 1, 1, 0, Vec::new(), emit)),
        Err(msg) if msg == "cancelled" => {
            Err(test_error("cancelled", "Archive test was cancelled."))
        }
        Err(msg) => Ok(finish_summary(
            operation_id,
            1,
            0,
            1,
            vec![TestFailure {
                path: label.into(),
                message: msg,
            }],
            emit,
        )),
    }
}

fn test_single_stream_gzip(
    path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let file = File::open(path)
        .map_err(|e| test_error("invalid_archive", format!("Cannot open gzip: {e}")))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "payload".into());
    test_single_named(operation_id, cancelled, &name, GzDecoder::new(file), emit)
}

fn test_single_stream_bzip2(
    path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let file = File::open(path)
        .map_err(|e| test_error("invalid_archive", format!("Cannot open bzip2: {e}")))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "payload".into());
    test_single_named(operation_id, cancelled, &name, BzDecoder::new(file), emit)
}

fn test_single_stream_xz(
    path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    let file = File::open(path)
        .map_err(|e| test_error("invalid_archive", format!("Cannot open xz: {e}")))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "payload".into());
    test_single_named(operation_id, cancelled, &name, XzDecoder::new(file), emit)
}

fn test_sevenz(
    path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    use sevenz_rust2::{ArchiveReader, Password};

    let mut reader = ArchiveReader::open(path, Password::empty())
        .map_err(|e| test_error("invalid_archive", format!("Cannot read 7z structure: {e}")))?;

    let total = reader
        .archive()
        .files
        .iter()
        .filter(|f| !f.is_anti_item && !f.is_directory)
        .count() as u64;
    let mut tested_ok = 0_u64;
    let mut tested_failed = 0_u64;
    let mut failures = Vec::new();
    let mut tested = 0_u64;
    let mut last = Instant::now()
        .checked_sub(PROGRESS_INTERVAL)
        .unwrap_or_else(Instant::now);

    let result = reader.for_each_entries(|entry, data| {
        if cancelled.load(Ordering::Relaxed) {
            return Err(sevenz_rust2::Error::Other("cancelled".into()));
        }
        if entry.is_anti_item {
            let mut sink = [0_u8; BUFFER_SIZE];
            loop {
                match data.read(&mut sink) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            return Ok(true);
        }
        if entry.is_directory {
            return Ok(true);
        }
        let name = entry.name().to_string();
        if last.elapsed() >= PROGRESS_INTERVAL {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: tested,
                total_files: total.max(1),
                current_file: name.clone(),
                percentage: if total == 0 {
                    100.0
                } else {
                    tested as f32 * 100.0 / total as f32
                },
                phase: None,
            });
            last = Instant::now();
        }
        let mut sink = [0_u8; BUFFER_SIZE];
        let mut ok = true;
        let mut err_msg = String::new();
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(sevenz_rust2::Error::Other("cancelled".into()));
            }
            match data.read(&mut sink) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    ok = false;
                    err_msg = e.to_string();
                    break;
                }
            }
        }
        tested += 1;
        if ok {
            tested_ok += 1;
        } else {
            tested_failed += 1;
            if failures.len() < MAX_FAILURES {
                failures.push(TestFailure {
                    path: name,
                    message: err_msg,
                });
            }
        }
        Ok(true)
    });

    if let Err(e) = result {
        let msg = e.to_string();
        if msg.contains("cancelled") {
            return Err(test_error("cancelled", "Archive test was cancelled."));
        }
        return Err(test_error(
            "invalid_archive",
            format!("7z test failed: {msg}"),
        ));
    }

    Ok(finish_summary(
        operation_id,
        tested,
        tested_ok,
        tested_failed,
        failures,
        emit,
    ))
}
