use crate::models::{CommandError, OperationProgress, TestArchiveSummary, TestFailure};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use zip::ZipArchive;

use crate::io_perf::{IO_BUFFER_SIZE as BUFFER_SIZE, PROGRESS_INTERVAL};
const MAX_FAILURES: usize = 20;

fn test_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

/// Stream-read every non-directory ZIP entry to verify decompression and CRC.
/// Does not write anything to disk.
///
/// Single pass over the central directory: skips dirs, tests file bodies.
/// Progress `total_files` uses `archive.len()` as an upper bound (includes
/// directory entries); the final summary reports the accurate non-dir count.
pub fn test_archive(
    zip_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<TestArchiveSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(test_error("invalid_operation", "Operation ID is empty."));
    }
    if !zip_path.is_file() {
        return Err(test_error("not_found", "Source ZIP file does not exist."));
    }

    let file = File::open(zip_path)
        .map_err(|error| test_error("invalid_archive", format!("Cannot open archive: {error}")))?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        test_error(
            "invalid_archive",
            format!("Cannot read ZIP structure: {error}"),
        )
    })?;

    // Upper bound for progress (includes directory entries).
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
            });
            last_progress = Instant::now();
        }

        let mut buffer = [0_u8; BUFFER_SIZE];
        let mut entry_ok = true;
        let mut fail_message = String::new();
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(test_error("cancelled", "Archive test was cancelled."));
            }
            match entry.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => {
                    entry_ok = false;
                    fail_message = error.to_string();
                    break;
                }
            }
        }

        tested += 1;
        if entry_ok {
            tested_ok += 1;
        } else {
            tested_failed += 1;
            if failures.len() < MAX_FAILURES {
                failures.push(TestFailure {
                    path: name,
                    message: fail_message,
                });
            }
        }
    }

    // Final progress and summary use accurate non-dir count.
    let total_entries = tested;
    emit(OperationProgress {
        operation_id: operation_id.into(),
        extracted_files: tested,
        total_files: total_entries,
        current_file: "Completed".into(),
        percentage: 100.0,
    });

    Ok(TestArchiveSummary {
        operation_id: operation_id.into(),
        total_entries,
        tested_ok,
        tested_failed,
        failures,
    })
}
